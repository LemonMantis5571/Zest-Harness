//! Shared background-job registry.
//!
//! Bash used to keep a private vector of child processes. That made a server
//! disappear when the tool instance was rebuilt and gave the model no way to
//! inspect output after the initial start response. This registry owns the
//! process, bounded output tail, terminal state, and completion notifications
//! independently of any one tool invocation.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::{broadcast, Mutex as AsyncMutex, Notify};

const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_RUNNING_JOBS: usize = 32;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Running,
    Stopping,
    Completed,
    Killed,
    Failed,
}

impl JobStatus {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Killed | Self::Failed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    pub id: String,
    pub kind: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_thread_id: Option<String>,
    /// Workspace that owns the process. Keeping this beside the logical owner
    /// lets a completion notice be written even if the window navigated away
    /// or the idle session was evicted before the child exited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_root: Option<String>,
    pub status: JobStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub started_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Completion notices are marked once they have been injected into the
    /// owning thread, avoiding duplicate wakeups after a reconnect.
    #[serde(default)]
    pub reported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRead {
    pub snapshot: JobSnapshot,
    pub text: String,
    pub next_offset: u64,
    pub truncated: bool,
}

pub type JobOutput = JobRead;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobEvent {
    Started(JobSnapshot),
    Output { id: String, next_offset: u64 },
    Changed(JobSnapshot),
    Completed(JobSnapshot),
}

#[derive(Debug)]
struct OutputBuffer {
    bytes: VecDeque<u8>,
    base_offset: u64,
    total_seen: u64,
    truncated: bool,
}

impl Default for OutputBuffer {
    fn default() -> Self {
        Self {
            bytes: VecDeque::new(),
            base_offset: 0,
            total_seen: 0,
            truncated: false,
        }
    }
}

impl OutputBuffer {
    fn append(&mut self, bytes: &[u8]) -> u64 {
        self.total_seen = self.total_seen.saturating_add(bytes.len() as u64);
        self.bytes.extend(bytes.iter().copied());
        while self.bytes.len() > MAX_OUTPUT_BYTES {
            self.bytes.pop_front();
            self.base_offset = self.base_offset.saturating_add(1);
            self.truncated = true;
        }
        self.base_offset.saturating_add(self.bytes.len() as u64)
    }

    fn read_from(&self, offset: u64) -> (String, u64, bool) {
        let effective = offset.max(self.base_offset);
        let start = effective.saturating_sub(self.base_offset) as usize;
        let text = String::from_utf8_lossy(
            self.bytes
                .iter()
                .skip(start.min(self.bytes.len()))
                .copied()
                .collect::<Vec<_>>()
                .as_slice(),
        )
        .to_string();
        (
            text,
            self.base_offset + self.bytes.len() as u64,
            offset < self.base_offset,
        )
    }
}

struct JobRecord {
    snapshot: Mutex<JobSnapshot>,
    child: AsyncMutex<Option<tokio::process::Child>>,
    output: Mutex<OutputBuffer>,
    output_notify: Notify,
    done_notify: Notify,
    kill_requested: AtomicBool,
}

struct Inner {
    jobs: Mutex<HashMap<String, Arc<JobRecord>>>,
    next_id: AtomicU64,
    events: broadcast::Sender<JobEvent>,
}

/// Process-wide registry shared by all runtimes in a desktop process.
#[derive(Clone)]
pub struct JobRegistry {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for JobRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobRegistry")
            .field("jobs", &self.list(None).len())
            .finish()
    }
}

impl Default for JobRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl JobRegistry {
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(128);
        Self {
            inner: Arc::new(Inner {
                jobs: Mutex::new(HashMap::new()),
                next_id: AtomicU64::new(1),
                events,
            }),
        }
    }

    /// Start a shell process and return as soon as the process is registered.
    /// The registry drains both pipes in the watcher, so a verbose build can
    /// never deadlock behind a full stdout/stderr pipe.
    pub async fn start_process(
        &self,
        command: &str,
        cwd: &Path,
        kind: impl Into<String>,
        label: impl Into<String>,
        owner_thread_id: Option<String>,
    ) -> Result<JobSnapshot, String> {
        let running = self.count_running(owner_thread_id.as_deref());
        if running >= MAX_RUNNING_JOBS {
            return Err(format!(
                "too many background jobs are already running (max {MAX_RUNNING_JOBS})"
            ));
        }

        let mut process = shell_command(command);
        process
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            process.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = process
            .spawn()
            .map_err(|error| format!("cannot start background job `{command}`: {error}"))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let snapshot = JobSnapshot {
            id: format!("job-{}", self.inner.next_id.fetch_add(1, Ordering::Relaxed)),
            kind: kind.into(),
            label: label.into(),
            owner_thread_id,
            owner_root: Some(cwd.to_string_lossy().into_owned()),
            status: JobStatus::Running,
            detail: None,
            started_at: now_secs(),
            finished_at: None,
            pid: child.id(),
            reported: false,
        };
        let record = Arc::new(JobRecord {
            snapshot: Mutex::new(snapshot.clone()),
            child: AsyncMutex::new(Some(child)),
            output: Mutex::new(OutputBuffer::default()),
            output_notify: Notify::new(),
            done_notify: Notify::new(),
            kill_requested: AtomicBool::new(false),
        });
        self.inner
            .jobs
            .lock()
            .map_err(|_| "job registry is unavailable".to_string())?
            .insert(snapshot.id.clone(), record.clone());
        let _ = self.inner.events.send(JobEvent::Started(snapshot.clone()));

        let registry = self.clone();
        let id = snapshot.id.clone();
        tokio::spawn(async move {
            watch_process(registry, id, record, stdout, stderr).await;
        });
        Ok(snapshot)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<JobEvent> {
        self.inner.events.subscribe()
    }

    pub fn list(&self, owner_thread_id: Option<&str>) -> Vec<JobSnapshot> {
        let mut jobs = self
            .inner
            .jobs
            .lock()
            .map(|jobs| {
                jobs.values()
                    .filter_map(|job| snapshot_if_owned(job, owner_thread_id))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        jobs.sort_by(|a, b| {
            b.started_at
                .cmp(&a.started_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        jobs
    }

    pub async fn snapshot(
        &self,
        id: &str,
        owner_thread_id: Option<&str>,
    ) -> Result<JobSnapshot, String> {
        let record = self.record(id, owner_thread_id)?;
        record
            .snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .map_err(|_| "job state is unavailable".into())
    }

    pub async fn read(
        &self,
        id: &str,
        owner_thread_id: Option<&str>,
        offset: u64,
    ) -> Result<JobRead, String> {
        let record = self.record(id, owner_thread_id)?;
        let snapshot = record
            .snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .map_err(|_| "job state is unavailable".to_string())?;
        let (text, next_offset, truncated) = record
            .output
            .lock()
            .map(|output| output.read_from(offset))
            .map_err(|_| "job output is unavailable".to_string())?;
        Ok(JobRead {
            snapshot,
            text,
            next_offset,
            truncated,
        })
    }

    pub async fn read_wait(
        &self,
        id: &str,
        owner_thread_id: Option<&str>,
        offset: u64,
        timeout: Option<Duration>,
    ) -> Result<JobRead, String> {
        let record = self.record(id, owner_thread_id)?;
        let has_new_output = || {
            record
                .output
                .lock()
                .map(|output| output.base_offset + output.bytes.len() as u64 > offset)
                .unwrap_or(true)
        };
        let wait = record.output_notify.notified();
        if !has_new_output()
            && !record
                .snapshot
                .lock()
                .map(|snapshot| snapshot.status.terminal())
                .unwrap_or(true)
        {
            if let Some(timeout) = timeout {
                let _ = tokio::time::timeout(timeout, wait).await;
            } else {
                wait.await;
            }
        }
        self.read(id, owner_thread_id, offset).await
    }

    pub async fn wait(
        &self,
        id: &str,
        owner_thread_id: Option<&str>,
        timeout: Option<Duration>,
    ) -> Result<JobSnapshot, String> {
        let record = self.record(id, owner_thread_id)?;
        let wait = record.done_notify.notified();
        if record
            .snapshot
            .lock()
            .map(|snapshot| snapshot.status.terminal())
            .unwrap_or(true)
        {
            return self.snapshot(id, owner_thread_id).await;
        }
        if let Some(timeout) = timeout {
            let _ = tokio::time::timeout(timeout, wait).await;
        } else {
            wait.await;
        }
        self.snapshot(id, owner_thread_id).await
    }

    pub async fn kill(
        &self,
        id: &str,
        owner_thread_id: Option<&str>,
        reason: Option<&str>,
    ) -> Result<JobSnapshot, String> {
        let record = self.record(id, owner_thread_id)?;
        let should_kill = {
            let mut snapshot = record
                .snapshot
                .lock()
                .map_err(|_| "job state is unavailable".to_string())?;
            if snapshot.status.terminal() {
                false
            } else {
                snapshot.status = JobStatus::Stopping;
                snapshot.detail = reason.map(str::to_string);
                record.kill_requested.store(true, Ordering::SeqCst);
                let _ = self.inner.events.send(JobEvent::Changed(snapshot.clone()));
                true
            }
        };
        if should_kill {
            // The watcher owns the child mutex while awaiting `wait()`. Do
            // not wait on that mutex here or a long-lived dev server could
            // never be cancelled. Kill by the fenced PID first; the watcher
            // then observes the exit and publishes the single terminal event.
            let pid = record
                .snapshot
                .lock()
                .ok()
                .and_then(|snapshot| snapshot.pid);
            if let Some(pid) = pid {
                terminate_process_tree(pid);
            } else {
                let mut child = record.child.lock().await;
                if let Some(child) = child.as_mut() {
                    let _ = child.start_kill();
                }
            }
        }
        self.wait(id, owner_thread_id, Some(Duration::from_secs(5)))
            .await
    }

    pub async fn mark_reported(
        &self,
        id: &str,
        owner_thread_id: Option<&str>,
    ) -> Result<bool, String> {
        let record = self.record(id, owner_thread_id)?;
        let mut snapshot = record
            .snapshot
            .lock()
            .map_err(|_| "job state is unavailable".to_string())?;
        if snapshot.reported {
            return Ok(false);
        }
        snapshot.reported = true;
        Ok(true)
    }

    pub fn count_running(&self, owner_thread_id: Option<&str>) -> usize {
        self.list(owner_thread_id)
            .into_iter()
            .filter(|snapshot| matches!(snapshot.status, JobStatus::Running | JobStatus::Stopping))
            .count()
    }

    fn record(&self, id: &str, owner_thread_id: Option<&str>) -> Result<Arc<JobRecord>, String> {
        let record = self
            .inner
            .jobs
            .lock()
            .map_err(|_| "job registry is unavailable".to_string())?
            .get(id)
            .cloned()
            .ok_or_else(|| format!("job `{id}` was not found"))?;
        if snapshot_if_owned(&record, owner_thread_id).is_none() {
            return Err(format!("job `{id}` was not found"));
        }
        Ok(record)
    }
}

fn snapshot_if_owned(record: &JobRecord, owner_thread_id: Option<&str>) -> Option<JobSnapshot> {
    let snapshot = record.snapshot.lock().ok()?.clone();
    match (owner_thread_id, snapshot.owner_thread_id.as_deref()) {
        (Some(wanted), Some(actual)) if wanted == actual => Some(snapshot),
        (None, _) => Some(snapshot),
        _ => None,
    }
}

async fn watch_process(
    registry: JobRegistry,
    id: String,
    record: Arc<JobRecord>,
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
) {
    let output_record = record.clone();
    let output_id = id.clone();
    let out_task = tokio::spawn(pump_output(
        stdout,
        output_record.clone(),
        registry.clone(),
        output_id.clone(),
    ));
    let err_task = tokio::spawn(pump_output(
        stderr,
        output_record,
        registry.clone(),
        output_id.clone(),
    ));

    let status = {
        let mut child = record.child.lock().await;
        match child.as_mut() {
            Some(child) => child.wait().await.ok(),
            None => None,
        }
    };
    let _ = out_task.await;
    let _ = err_task.await;

    let kill_requested = record.kill_requested.load(Ordering::SeqCst);
    let final_status = if kill_requested {
        JobStatus::Killed
    } else if status.as_ref().is_some_and(|status| status.success()) {
        JobStatus::Completed
    } else {
        JobStatus::Failed
    };
    let snapshot = {
        let mut snapshot = match record.snapshot.lock() {
            Ok(snapshot) => snapshot,
            Err(_) => return,
        };
        if snapshot.status.terminal() {
            snapshot.clone()
        } else {
            snapshot.status = final_status;
            snapshot.finished_at = Some(now_secs());
            if snapshot.detail.is_none() && final_status == JobStatus::Failed {
                snapshot.detail = Some(match status.and_then(|status| status.code()) {
                    Some(code) => format!("process exited with status {code}"),
                    None => "process exited without a status".into(),
                });
            }
            snapshot.clone()
        }
    };
    let _ = registry.inner.events.send(JobEvent::Completed(snapshot));
    record.output_notify.notify_waiters();
    record.done_notify.notify_waiters();
}

async fn pump_output<R>(
    reader: Option<R>,
    record: Arc<JobRecord>,
    registry: JobRegistry,
    id: String,
) where
    R: AsyncRead + Unpin,
{
    let Some(mut reader) = reader else {
        return;
    };
    let mut buffer = [0u8; 8 * 1024];
    loop {
        let count = match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        let next_offset = record
            .output
            .lock()
            .map(|mut output| output.append(&buffer[..count]))
            .unwrap_or(0);
        record.output_notify.notify_waiters();
        let _ = registry.inner.events.send(JobEvent::Output {
            id: id.clone(),
            next_offset,
        });
    }
}

#[cfg(windows)]
fn shell_command(command: &str) -> Command {
    let mut cmd = Command::new("cmd");
    cmd.arg("/C").arg(command);
    cmd
}

#[cfg(not(windows))]
fn shell_command(command: &str) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command);
    cmd
}

#[cfg(windows)]
fn terminate_process_tree(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status();
}

#[cfg(not(windows))]
fn terminate_process_tree(pid: u32) {
    let _ = std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command() -> &'static str {
        if cfg!(windows) {
            "echo first & echo second"
        } else {
            "printf first; printf second"
        }
    }

    #[tokio::test]
    async fn registry_keeps_incremental_output_and_terminal_state() {
        let registry = JobRegistry::new();
        let job = registry
            .start_process(command(), Path::new("."), "test", "output", None)
            .await
            .unwrap();
        let done = registry
            .wait(&job.id, None, Some(Duration::from_secs(5)))
            .await
            .unwrap();
        assert_eq!(done.status, JobStatus::Completed);
        let first = registry.read(&job.id, None, 0).await.unwrap();
        assert!(first.text.contains("first"), "{}", first.text);
        let second = registry
            .read(&job.id, None, first.next_offset)
            .await
            .unwrap();
        assert!(second.text.is_empty());
    }

    #[tokio::test]
    async fn owner_fencing_hides_other_threads() {
        let registry = JobRegistry::new();
        let job = registry
            .start_process(
                command(),
                Path::new("."),
                "test",
                "owned",
                Some("thread-a".into()),
            )
            .await
            .unwrap();
        assert!(registry.list(Some("thread-b")).is_empty());
        assert!(registry.read(&job.id, Some("thread-b"), 0).await.is_err());
        registry
            .wait(&job.id, Some("thread-a"), Some(Duration::from_secs(5)))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn kill_has_one_terminal_outcome() {
        let registry = JobRegistry::new();
        let command = if cfg!(windows) {
            "ping 127.0.0.1 -n 10"
        } else {
            "sleep 10"
        };
        let job = registry
            .start_process(command, Path::new("."), "test", "kill", None)
            .await
            .unwrap();
        let killed = registry
            .kill(&job.id, None, Some("test cancellation"))
            .await
            .unwrap();
        assert_eq!(killed.status, JobStatus::Killed);
        assert!(registry
            .kill(&job.id, None, Some("second cancellation"))
            .await
            .unwrap()
            .status
            .terminal());
    }
}
