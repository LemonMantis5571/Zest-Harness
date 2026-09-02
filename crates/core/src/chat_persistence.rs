//! Durable chat run and interrupt state.
//!
//! This is a small, local-first persistence layer for chat stores. Thread JSON
//! remains the authoritative transcript; these records
//! answer the separate lifecycle questions: is a turn still running, did it
//! pause for approval, and how did it finish?
//!
//! The store contracts intentionally preserve these useful invariants:
//!
//! - `create_or_resume` never overwrites an existing run.
//! - Updating an unknown run is a no-op.
//! - An interrupt is insert-if-absent and can only move from `pending` to a
//!   terminal state.
//! - Active-run lookup is by stable `thread_id`, not by an ephemeral run id.
//!
//! This implementation still does not resume a provider stream after a process
//! restart. Providers default to unsupported, but the run can now carry a
//! non-secret provider handle so an explicitly capable provider can opt in later
//! without changing thread history.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::anthropic::types::Usage;
use crate::error::{HarnessError, Result};
use crate::fsutil;
use crate::provider::ResumeHandle;
use crate::thread::{Thread, ThreadStore};

// Zest is a single local desktop process, but approval commands and the stream
// callback can still update the same record concurrently. This process-local
// mutex covers that race for the first local implementation. Atomic file
// replacement still protects the file if another process touches it.
static PROJECT_STATE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn state_lock() -> Result<MutexGuard<'static, ()>> {
    PROJECT_STATE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| HarnessError::Other("chat persistence lock poisoned".into()))
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn validate_id(raw: &str, kind: &str) -> Result<String> {
    if raw.is_empty() {
        return Err(HarnessError::Other(format!("{kind} id must not be empty")));
    }
    if raw.len() > 200 {
        return Err(HarnessError::Other(format!("{kind} id is too long")));
    }
    if !raw
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(HarnessError::Other(format!(
            "{kind} id may only contain ASCII letters, digits, '-' and '_'"
        )));
    }
    Ok(raw.to_string())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path, kind: &str, id: &str) -> Result<Option<T>> {
    let body = match fs::read_to_string(path) {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(HarnessError::Other(format!(
                "read {kind} {id} {}: {error}",
                path.display()
            )))
        }
    };
    serde_json::from_str(&body).map(Some).map_err(|error| {
        HarnessError::Other(format!(
            "{kind} {id} is corrupt at {}: {error}",
            path.display()
        ))
    })
}

fn write_json<T: Serialize>(path: &Path, kind: &str, id: &str, value: &T) -> Result<()> {
    fsutil::atomic_write_json(path, value).map_err(|error| {
        HarnessError::Other(format!("write {kind} {id} {}: {error}", path.display()))
    })
}

/// Lifecycle state for one assistant turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    /// A human/tool wait is active. This is not terminal.
    Interrupted,
    Completed,
    Failed,
    Aborted,
}

impl RunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Aborted)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunError {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// Provider usage copied into a run record without making the run store depend
/// on a provider-specific wire type at read time.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_creation_input_tokens: u32,
    pub cache_read_input_tokens: u32,
}

impl From<&Usage> for RunUsage {
    fn from(usage: &Usage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_creation_input_tokens: usage.cache_creation_input_tokens,
            cache_read_input_tokens: usage.cache_read_input_tokens,
        }
    }
}

/// Durable bookkeeping for one turn within a thread.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub run_id: String,
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// UI message that submitted this run. Keeping the identity here lets a
    /// desktop recover the exact turn without guessing from transcript order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_message_id: Option<String>,
    /// Assistant projection created for this run, useful for future replay /
    /// resume adapters and diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_message_id: Option<String>,
    pub status: RunStatus,
    pub started_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RunError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<RunUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_handle: Option<ResumeHandle>,
}

/// Partial mutable fields for a run update. An absent field is left unchanged.
#[derive(Debug, Clone, Default)]
pub struct RunPatch {
    pub status: Option<RunStatus>,
    pub finished_at: Option<u64>,
    pub error: Option<RunError>,
    pub usage: Option<RunUsage>,
    pub resume_handle: Option<ResumeHandle>,
}

/// `<workspace>/.zest/runs`.
#[derive(Debug, Clone)]
pub struct RunStore {
    dir: PathBuf,
}

impl RunStore {
    pub fn open(workspace_root: impl AsRef<Path>) -> Result<Self> {
        let dir = workspace_root.as_ref().join(".zest").join("runs");
        fs::create_dir_all(&dir).map_err(|error| {
            HarnessError::Other(format!("create run dir {}: {error}", dir.display()))
        })?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path_for(&self, run_id: &str) -> Result<PathBuf> {
        Ok(self
            .dir
            .join(format!("{}.json", validate_id(run_id, "run")?)))
    }

    pub fn load(&self, run_id: &str) -> Result<Option<RunRecord>> {
        let run_id = validate_id(run_id, "run")?;
        read_json(&self.path_for(&run_id)?, "run", &run_id)
    }

    /// Create a running record, or return the existing record unchanged.
    pub fn create_or_resume(&self, run_id: &str, thread_id: &str) -> Result<RunRecord> {
        self.create_or_resume_with_provider_and_messages(run_id, thread_id, None, None, None)
    }

    /// Create a running record with the provider identity needed by a future
    /// provider-specific resume implementation.
    pub fn create_or_resume_for_provider(
        &self,
        run_id: &str,
        thread_id: &str,
        provider_id: &str,
    ) -> Result<RunRecord> {
        let provider_id = (!provider_id.trim().is_empty()).then(|| provider_id.to_string());
        self.create_or_resume_with_provider_and_messages(run_id, thread_id, provider_id, None, None)
    }

    /// Create a running record and bind it to the two transcript projections
    /// emitted for this turn. Existing records are returned unchanged, so a
    /// retry cannot overwrite an older run's identity.
    pub fn create_or_resume_for_turn(
        &self,
        run_id: &str,
        thread_id: &str,
        provider_id: &str,
        user_message_id: &str,
        assistant_message_id: &str,
    ) -> Result<RunRecord> {
        let provider_id = (!provider_id.trim().is_empty()).then(|| provider_id.to_string());
        self.create_or_resume_with_provider_and_messages(
            run_id,
            thread_id,
            provider_id,
            Some(user_message_id),
            Some(assistant_message_id),
        )
    }

    fn create_or_resume_with_provider_and_messages(
        &self,
        run_id: &str,
        thread_id: &str,
        provider_id: Option<String>,
        user_message_id: Option<&str>,
        assistant_message_id: Option<&str>,
    ) -> Result<RunRecord> {
        let run_id = validate_id(run_id, "run")?;
        let thread_id = validate_id(thread_id, "thread")?;
        let user_message_id = user_message_id
            .map(|id| validate_id(id, "user message"))
            .transpose()?;
        let assistant_message_id = assistant_message_id
            .map(|id| validate_id(id, "assistant message"))
            .transpose()?;
        let _guard = state_lock()?;
        if let Some(existing) = self.load(&run_id)? {
            return Ok(existing);
        }

        let record = RunRecord {
            run_id: run_id.clone(),
            thread_id,
            provider_id,
            user_message_id,
            assistant_message_id,
            status: RunStatus::Running,
            started_at: now_millis(),
            finished_at: None,
            error: None,
            usage: None,
            resume_handle: None,
        };
        write_json(&self.path_for(&run_id)?, "run", &run_id, &record)?;
        Ok(record)
    }

    /// Patch a known run. Unknown run ids intentionally do nothing.
    pub fn update(&self, run_id: &str, patch: RunPatch) -> Result<()> {
        let run_id = validate_id(run_id, "run")?;
        let _guard = state_lock()?;
        let Some(mut record) = self.load(&run_id)? else {
            return Ok(());
        };
        if let Some(status) = patch.status {
            record.status = status;
        }
        if let Some(finished_at) = patch.finished_at {
            record.finished_at = Some(finished_at);
        }
        if let Some(error) = patch.error {
            record.error = Some(error);
        }
        if let Some(usage) = patch.usage {
            record.usage = Some(usage);
        }
        if let Some(resume_handle) = patch.resume_handle {
            record.resume_handle = Some(resume_handle);
        }
        write_json(&self.path_for(&run_id)?, "run", &run_id, &record)
    }

    pub fn mark_running(&self, run_id: &str) -> Result<()> {
        self.update(
            run_id,
            RunPatch {
                status: Some(RunStatus::Running),
                ..RunPatch::default()
            },
        )
    }

    pub fn mark_interrupted(&self, run_id: &str) -> Result<()> {
        self.update(
            run_id,
            RunPatch {
                status: Some(RunStatus::Interrupted),
                ..RunPatch::default()
            },
        )
    }

    pub fn mark_completed(&self, run_id: &str, usage: Option<&Usage>) -> Result<()> {
        self.update(
            run_id,
            RunPatch {
                status: Some(RunStatus::Completed),
                finished_at: Some(now_millis()),
                usage: usage.map(RunUsage::from),
                resume_handle: None,
                ..RunPatch::default()
            },
        )
    }

    pub fn mark_failed(&self, run_id: &str, error: impl Into<String>) -> Result<()> {
        self.update(
            run_id,
            RunPatch {
                status: Some(RunStatus::Failed),
                finished_at: Some(now_millis()),
                error: Some(RunError {
                    message: error.into(),
                    code: None,
                }),
                resume_handle: None,
                ..RunPatch::default()
            },
        )
    }

    pub fn mark_aborted(&self, run_id: &str) -> Result<()> {
        self.update(
            run_id,
            RunPatch {
                status: Some(RunStatus::Aborted),
                finished_at: Some(now_millis()),
                resume_handle: None,
                ..RunPatch::default()
            },
        )
    }

    /// Persist the latest provider checkpoint for a still-running turn.
    pub fn set_resume_handle(&self, run_id: &str, resume_handle: ResumeHandle) -> Result<()> {
        self.update(
            run_id,
            RunPatch {
                resume_handle: Some(resume_handle),
                ..RunPatch::default()
            },
        )
    }

    /// Remove every run belonging to a thread. Best effort by design — see
    /// [].
    pub fn delete_for_thread(&self, thread_id: &str) -> usize {
        let Ok(runs) = self.list_by_thread(thread_id) else {
            return 0;
        };
        let _guard = state_lock().ok();
        runs.iter()
            .filter(|run| {
                self.path_for(&run.run_id)
                    .map(|path| fs::remove_file(path).is_ok())
                    .unwrap_or(false)
            })
            .count()
    }

    pub fn list_by_thread(&self, thread_id: &str) -> Result<Vec<RunRecord>> {
        let thread_id = validate_id(thread_id, "thread")?;
        let _guard = state_lock()?;
        let mut runs = Vec::new();
        for entry in fs::read_dir(&self.dir).map_err(|error| {
            HarnessError::Other(format!("list runs {}: {error}", self.dir.display()))
        })? {
            let entry = entry.map_err(|error| HarnessError::Other(error.to_string()))?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if let Some(run) = self.load(id)? {
                if run.thread_id == thread_id {
                    runs.push(run);
                }
            }
        }
        runs.sort_by(|left, right| {
            left.started_at
                .cmp(&right.started_at)
                .then_with(|| left.run_id.cmp(&right.run_id))
        });
        Ok(runs)
    }

    /// Find the newest still-running run for a stable thread id.
    pub fn find_active_run(&self, thread_id: &str) -> Result<Option<RunRecord>> {
        Ok(self
            .list_by_thread(thread_id)?
            .into_iter()
            .filter(|run| run.status == RunStatus::Running)
            .max_by(|left, right| {
                left.started_at
                    .cmp(&right.started_at)
                    .then_with(|| left.run_id.cmp(&right.run_id))
            }))
    }

    /// Find the newest unfinished run whose submitted user message is still
    /// present in the thread transcript. This is the safe local fallback when
    /// a provider cannot resume its old stream: offer a fresh run for the exact
    /// same prompt instead of asking the UI to infer it from array position.
    pub fn find_recoverable_run(&self, thread_id: &str) -> Result<Option<RecoverableRun>> {
        let recoverable = self
            .list_by_thread(thread_id)?
            .into_iter()
            .filter(|run| !run.status.is_terminal())
            .filter_map(|run| {
                run.user_message_id.map(|user_message_id| {
                    (
                        run.started_at,
                        RecoverableRun {
                            run_id: run.run_id,
                            user_message_id,
                        },
                    )
                })
            })
            .max_by(|left, right| {
                left.0
                    .cmp(&right.0)
                    .then_with(|| left.1.run_id.cmp(&right.1.run_id))
            })
            .map(|(_, recoverable)| recoverable);
        Ok(recoverable)
    }
}

/// Lifecycle state for a human/tool wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptStatus {
    Pending,
    Resolved,
    Cancelled,
}

impl InterruptStatus {
    pub fn is_pending(self) -> bool {
        self == Self::Pending
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterruptRecord {
    pub interrupt_id: String,
    pub run_id: String,
    pub thread_id: String,
    pub status: InterruptStatus,
    pub requested_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<u64>,
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<Value>,
}

/// `<workspace>/.zest/interrupts`.
#[derive(Debug, Clone)]
pub struct InterruptStore {
    dir: PathBuf,
}

impl InterruptStore {
    pub fn open(workspace_root: impl AsRef<Path>) -> Result<Self> {
        let dir = workspace_root.as_ref().join(".zest").join("interrupts");
        fs::create_dir_all(&dir).map_err(|error| {
            HarnessError::Other(format!("create interrupt dir {}: {error}", dir.display()))
        })?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path_for(&self, interrupt_id: &str) -> Result<PathBuf> {
        Ok(self
            .dir
            .join(format!("{}.json", validate_id(interrupt_id, "interrupt")?)))
    }

    pub fn load(&self, interrupt_id: &str) -> Result<Option<InterruptRecord>> {
        let interrupt_id = validate_id(interrupt_id, "interrupt")?;
        read_json(&self.path_for(&interrupt_id)?, "interrupt", &interrupt_id)
    }

    /// Insert a pending interrupt. A duplicate id returns the existing record
    /// unchanged, so retries cannot resurrect a resolved approval.
    pub fn create(
        &self,
        interrupt_id: &str,
        run_id: &str,
        thread_id: &str,
        payload: Value,
    ) -> Result<InterruptRecord> {
        let interrupt_id = validate_id(interrupt_id, "interrupt")?;
        let run_id = validate_id(run_id, "run")?;
        let thread_id = validate_id(thread_id, "thread")?;
        let _guard = state_lock()?;
        if let Some(existing) = self.load(&interrupt_id)? {
            return Ok(existing);
        }

        let record = InterruptRecord {
            interrupt_id: interrupt_id.clone(),
            run_id,
            thread_id,
            status: InterruptStatus::Pending,
            requested_at: now_millis(),
            resolved_at: None,
            payload,
            response: None,
        };
        write_json(
            &self.path_for(&interrupt_id)?,
            "interrupt",
            &interrupt_id,
            &record,
        )?;
        Ok(record)
    }

    fn finish(
        &self,
        interrupt_id: &str,
        status: InterruptStatus,
        response: Option<Value>,
    ) -> Result<()> {
        let interrupt_id = validate_id(interrupt_id, "interrupt")?;
        let _guard = state_lock()?;
        let Some(mut record) = self.load(&interrupt_id)? else {
            return Ok(());
        };
        if !record.status.is_pending() {
            return Ok(());
        }
        record.status = status;
        record.resolved_at = Some(now_millis());
        record.response = response;
        write_json(
            &self.path_for(&interrupt_id)?,
            "interrupt",
            &interrupt_id,
            &record,
        )
    }

    pub fn resolve(&self, interrupt_id: &str, response: Option<Value>) -> Result<()> {
        self.finish(interrupt_id, InterruptStatus::Resolved, response)
    }

    pub fn cancel(&self, interrupt_id: &str) -> Result<()> {
        self.finish(interrupt_id, InterruptStatus::Cancelled, None)
    }

    /// Remove every interrupt belonging to a thread. Best effort by design.
    pub fn delete_for_thread(&self, thread_id: &str) -> usize {
        let Ok(interrupts) = self.list(thread_id) else {
            return 0;
        };
        interrupts
            .iter()
            .filter(|interrupt| {
                self.path_for(&interrupt.interrupt_id)
                    .map(|path| fs::remove_file(path).is_ok())
                    .unwrap_or(false)
            })
            .count()
    }

    pub fn list(&self, thread_id: &str) -> Result<Vec<InterruptRecord>> {
        let thread_id = validate_id(thread_id, "thread")?;
        let mut interrupts = self.read_all()?;
        interrupts.retain(|interrupt| interrupt.thread_id == thread_id);
        interrupts.sort_by(|left, right| {
            left.requested_at
                .cmp(&right.requested_at)
                .then_with(|| left.interrupt_id.cmp(&right.interrupt_id))
        });
        Ok(interrupts)
    }

    pub fn list_pending(&self, thread_id: &str) -> Result<Vec<InterruptRecord>> {
        Ok(self
            .list(thread_id)?
            .into_iter()
            .filter(|interrupt| interrupt.status.is_pending())
            .collect())
    }

    pub fn list_by_run(&self, run_id: &str) -> Result<Vec<InterruptRecord>> {
        let run_id = validate_id(run_id, "run")?;
        let mut interrupts = self.read_all()?;
        interrupts.retain(|interrupt| interrupt.run_id == run_id);
        interrupts.sort_by(|left, right| {
            left.requested_at
                .cmp(&right.requested_at)
                .then_with(|| left.interrupt_id.cmp(&right.interrupt_id))
        });
        Ok(interrupts)
    }

    pub fn list_pending_by_run(&self, run_id: &str) -> Result<Vec<InterruptRecord>> {
        Ok(self
            .list_by_run(run_id)?
            .into_iter()
            .filter(|interrupt| interrupt.status.is_pending())
            .collect())
    }

    pub fn cancel_pending_by_run(&self, run_id: &str) -> Result<()> {
        for interrupt in self.list_pending_by_run(run_id)? {
            self.cancel(&interrupt.interrupt_id)?;
        }
        Ok(())
    }

    fn read_all(&self) -> Result<Vec<InterruptRecord>> {
        let _guard = state_lock()?;
        let mut interrupts = Vec::new();
        for entry in fs::read_dir(&self.dir).map_err(|error| {
            HarnessError::Other(format!("list interrupts {}: {error}", self.dir.display()))
        })? {
            let entry = entry.map_err(|error| HarnessError::Other(error.to_string()))?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if let Some(interrupt) = self.load(id)? {
                interrupts.push(interrupt);
            }
        }
        Ok(interrupts)
    }
}

/// The two state stores used by Zest's chat turn lifecycle.
#[derive(Debug, Clone)]
pub struct ChatPersistence {
    pub runs: RunStore,
    pub interrupts: InterruptStore,
}

impl ChatPersistence {
    /// Remove the lifecycle records belonging to a deleted thread.
    ///
    /// Deleting a chat removed its transcript and left these behind, so a
    /// workspace accumulated run and interrupt records for conversations that no
    /// longer exist — read on every open, never collected, and describing
    /// something the user explicitly asked to be rid of.
    ///
    /// Returns how many records went, and never fails the deletion: the thread
    /// is already gone by the time this runs, and refusing to finish because a
    /// stale side record would not unlink helps nobody.
    pub fn forget_thread(&self, thread_id: &str) -> usize {
        self.runs.delete_for_thread(thread_id) + self.interrupts.delete_for_thread(thread_id)
    }
}

/// The durable chat projection needed after opening a workspace.
///
/// `Thread` remains the authoritative transcript and provider wire history.
/// The lifecycle fields are deliberately reconstructed beside it instead of
/// being folded into the transcript, so a future provider-resume capability can
/// use the same seam without changing the message format.
#[derive(Debug, Clone)]
pub struct ReconstructedChat {
    pub thread: Thread,
    pub thread_warning: Option<String>,
    pub active_run: Option<RunRecord>,
    /// The newest run that cannot be resumed by the current process but can be
    /// safely offered as a fresh retry from its persisted user message.
    pub recoverable_run: Option<RecoverableRun>,
    pub pending_interrupts: Vec<InterruptRecord>,
}

/// Minimal retry identity. The prompt itself remains in the authoritative
/// thread transcript instead of being duplicated in lifecycle JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverableRun {
    pub run_id: String,
    pub user_message_id: String,
}

/// Result of closing non-terminal lifecycle records that cannot be resumed by
/// the current provider runtime after a process restart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecoveryReconciliation {
    pub aborted_runs: usize,
    pub cancelled_interrupts: usize,
}

impl ChatPersistence {
    pub fn open(workspace_root: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            runs: RunStore::open(&workspace_root)?,
            interrupts: InterruptStore::open(workspace_root)?,
        })
    }

    /// Reconstruct a chat from its authoritative transcript plus lifecycle
    /// records. This is a read-oriented snapshot; it does not claim that the
    /// provider can resume the old stream.
    pub fn reconstruct_chat(
        &self,
        thread_store: &ThreadStore,
        thread_id: &str,
    ) -> Result<ReconstructedChat> {
        let loaded = thread_store.load_with_recovery(thread_id)?;
        self.reconstruct_chat_from_thread(loaded.thread, loaded.warning)
    }

    /// Same as [`Self::reconstruct_chat`] when the caller already holds the
    /// transcript, so opening a chat does not parse the JSON twice.
    pub fn reconstruct_chat_from_thread(
        &self,
        thread: Thread,
        thread_warning: Option<String>,
    ) -> Result<ReconstructedChat> {
        let thread_id = thread.id.clone();
        let recoverable_run = self
            .runs
            .find_recoverable_run(&thread_id)?
            .filter(|candidate| {
                thread
                    .messages
                    .iter()
                    .any(|message| message.id() == candidate.user_message_id)
            });
        Ok(ReconstructedChat {
            thread,
            thread_warning,
            active_run: self.runs.find_active_run(&thread_id)?,
            recoverable_run,
            pending_interrupts: self.interrupts.list_pending(&thread_id)?,
        })
    }

    /// Reconcile lifecycle records left by a previous process.
    ///
    /// Zest cannot yet send a durable resume request to a provider. Once the
    /// desktop has loaded the transcript, every `running` or `interrupted` run
    /// is therefore stale: close its pending waits and mark the run aborted so
    /// the next message starts from a truthful state.
    pub fn reconcile_after_restart(&self, thread_id: &str) -> Result<RecoveryReconciliation> {
        let mut result = RecoveryReconciliation::default();
        for run in self.runs.list_by_thread(thread_id)? {
            if run.status.is_terminal() {
                continue;
            }

            let pending = self.interrupts.list_pending_by_run(&run.run_id)?;
            for interrupt in pending {
                self.interrupts.cancel(&interrupt.interrupt_id)?;
                result.cancelled_interrupts += 1;
            }
            self.runs.mark_aborted(&run.run_id)?;
            result.aborted_runs += 1;
        }

        // A pending interrupt without a non-terminal run is inconsistent, but
        // it is still unsafe to expose it as actionable after a restart.
        for interrupt in self.interrupts.list_pending(thread_id)? {
            self.interrupts.cancel(&interrupt.interrupt_id)?;
            result.cancelled_interrupts += 1;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> crate::fsutil::ScratchDir {
        crate::fsutil::ScratchDir::new(&format!("zest-chat-persistence-{name}-"))
    }
    /// Deleting a chat used to leave its lifecycle records behind, read on
    /// every open and never collected, describing a conversation the user had
    /// explicitly asked to be rid of.
    #[test]
    fn forgetting_a_thread_removes_its_lifecycle_records() {
        let root = scratch("forget");
        let persistence = ChatPersistence::open(&root).unwrap();

        persistence
            .runs
            .create_or_resume("run-1", "thread-a")
            .unwrap();
        persistence
            .runs
            .create_or_resume("run-2", "thread-a")
            .unwrap();
        persistence
            .runs
            .create_or_resume("run-3", "thread-b")
            .unwrap();
        persistence
            .interrupts
            .create("int-1", "run-1", "thread-a", serde_json::json!({}))
            .unwrap();
        persistence
            .interrupts
            .create("int-2", "run-3", "thread-b", serde_json::json!({}))
            .unwrap();

        let removed = persistence.forget_thread("thread-a");
        assert_eq!(removed, 3, "two runs and one interrupt");

        assert!(persistence
            .runs
            .list_by_thread("thread-a")
            .unwrap()
            .is_empty());
        assert!(persistence.interrupts.list("thread-a").unwrap().is_empty());

        // The other conversation is untouched — deletion is scoped, not a sweep.
        assert_eq!(
            persistence.runs.list_by_thread("thread-b").unwrap().len(),
            1
        );
        assert_eq!(persistence.interrupts.list("thread-b").unwrap().len(), 1);
    }

    #[test]
    fn runs_are_idempotent_and_active_lookup_uses_thread_id() {
        let root = scratch("runs");
        let store = RunStore::open(&root).unwrap();
        let thread_id = "thread-1";

        let first = store.create_or_resume("run-1", thread_id).unwrap();
        let second = store.create_or_resume("run-2", thread_id).unwrap();
        assert_eq!(
            store.find_active_run(thread_id).unwrap().unwrap().run_id,
            second.run_id
        );

        store.mark_completed("run-1", None).unwrap();
        let existing = store.create_or_resume("run-1", thread_id).unwrap();
        assert_eq!(existing.status, RunStatus::Completed);
        assert!(store.find_active_run(thread_id).unwrap().is_some());

        store.mark_interrupted("run-2").unwrap();
        assert!(store.find_active_run(thread_id).unwrap().is_none());
        store.mark_running("run-2").unwrap();
        assert_eq!(
            store.find_active_run(thread_id).unwrap().unwrap().run_id,
            second.run_id
        );

        // Unknown updates are no-ops, matching the adapter contract.
        store.mark_aborted("run-unknown").unwrap();
        assert!(store.load("run-unknown").unwrap().is_none());
        assert_ne!(first.started_at, 0);
    }

    #[test]
    fn provider_resume_handles_round_trip_without_overwriting_the_run() {
        let root = scratch("resume-handle");
        let store = RunStore::open(&root).unwrap();

        let created = store
            .create_or_resume_for_provider("run-resume", "thread-resume", "codex")
            .unwrap();
        assert_eq!(created.provider_id.as_deref(), Some("codex"));
        assert!(created.resume_handle.is_none());

        let handle = ResumeHandle::new("provider-run-42").with_cursor("event-17");
        store
            .set_resume_handle("run-resume", handle.clone())
            .unwrap();
        let loaded = store.load("run-resume").unwrap().unwrap();
        assert_eq!(loaded.provider_id.as_deref(), Some("codex"));
        assert_eq!(loaded.resume_handle, Some(handle.clone()));

        // Idempotent creation returns the stored checkpoint instead of
        // replacing it with a fresh run record.
        let existing = store
            .create_or_resume_for_provider("run-resume", "thread-other", "other")
            .unwrap();
        assert_eq!(existing.thread_id, "thread-resume");
        assert_eq!(existing.provider_id.as_deref(), Some("codex"));
        assert_eq!(existing.resume_handle, Some(handle));
    }

    #[test]
    fn recoverable_run_keeps_the_exact_submitted_message_identity() {
        let root = scratch("recoverable-run");
        let store = RunStore::open(&root).unwrap();

        store
            .create_or_resume_for_turn(
                "run-recoverable",
                "thread-recoverable",
                "anthropic",
                "user-17",
                "assistant-17",
            )
            .unwrap();

        let record = store.load("run-recoverable").unwrap().unwrap();
        assert_eq!(record.user_message_id.as_deref(), Some("user-17"));
        assert_eq!(record.assistant_message_id.as_deref(), Some("assistant-17"));
        assert_eq!(
            store.find_recoverable_run("thread-recoverable").unwrap(),
            Some(RecoverableRun {
                run_id: "run-recoverable".into(),
                user_message_id: "user-17".into(),
            })
        );

        store.mark_aborted("run-recoverable").unwrap();
        assert!(store
            .find_recoverable_run("thread-recoverable")
            .unwrap()
            .is_none());
    }

    #[test]
    fn interrupts_are_insert_if_absent_and_terminal_transitions_are_sticky() {
        let root = scratch("interrupts");
        let store = InterruptStore::open(&root).unwrap();
        let payload = serde_json::json!({ "kind": "approval", "approved": false });

        let created = store
            .create("interrupt-1", "run-1", "thread-1", payload.clone())
            .unwrap();
        assert_eq!(created.status, InterruptStatus::Pending);
        assert_eq!(store.list_pending("thread-1").unwrap().len(), 1);

        store
            .resolve("interrupt-1", Some(serde_json::json!({ "approved": true })))
            .unwrap();
        let resolved = store.load("interrupt-1").unwrap().unwrap();
        assert_eq!(resolved.status, InterruptStatus::Resolved);
        assert!(resolved.resolved_at.is_some());

        // A retry cannot overwrite the resolved record or put it back to pending.
        let duplicate = store
            .create("interrupt-1", "run-other", "thread-other", payload)
            .unwrap();
        assert_eq!(duplicate.status, InterruptStatus::Resolved);
        assert_eq!(duplicate.run_id, "run-1");
        assert!(store.list_pending("thread-1").unwrap().is_empty());

        store.cancel("interrupt-missing").unwrap();
        assert!(store.list_by_run("run-1").unwrap().len() == 1);
    }

    #[test]
    fn chat_persistence_creates_both_project_scoped_stores() {
        let root = scratch("bundle");
        let persistence = ChatPersistence::open(&root).unwrap();
        assert!(persistence.runs.dir().is_dir());
        assert!(persistence.interrupts.dir().is_dir());
        assert!(persistence.runs.dir().starts_with(root.join(".zest")));
    }

    #[test]
    fn reconstructs_transcript_and_lifecycle_projection() {
        let root = scratch("reconstruct");
        let thread_store = ThreadStore::open(&root).unwrap();
        let thread = thread_store.create_for_provider("codex").unwrap();
        let persistence = ChatPersistence::open(&root).unwrap();

        let run = persistence
            .runs
            .create_or_resume("run-reconstruct", &thread.id)
            .unwrap();
        persistence
            .interrupts
            .create(
                "interrupt-reconstruct",
                &run.run_id,
                &thread.id,
                serde_json::json!({ "kind": "approval" }),
            )
            .unwrap();

        let reconstructed = persistence
            .reconstruct_chat(&thread_store, &thread.id)
            .unwrap();
        assert_eq!(reconstructed.thread.id, thread.id);
        assert_eq!(reconstructed.thread_warning, None);
        assert_eq!(
            reconstructed
                .active_run
                .as_ref()
                .map(|run| run.run_id.as_str()),
            Some("run-reconstruct")
        );
        assert_eq!(reconstructed.pending_interrupts.len(), 1);
        assert_eq!(
            reconstructed.pending_interrupts[0].interrupt_id,
            "interrupt-reconstruct"
        );
    }

    #[test]
    fn reconciliation_closes_stale_runs_and_waits() {
        let root = scratch("reconcile");
        let thread_store = ThreadStore::open(&root).unwrap();
        let thread = thread_store.create_for_provider("codex").unwrap();
        let persistence = ChatPersistence::open(&root).unwrap();

        let running = persistence
            .runs
            .create_or_resume("run-running", &thread.id)
            .unwrap();
        let interrupted = persistence
            .runs
            .create_or_resume("run-interrupted", &thread.id)
            .unwrap();
        persistence
            .runs
            .mark_interrupted(&interrupted.run_id)
            .unwrap();
        for (interrupt_id, run_id) in [
            ("interrupt-running", running.run_id.as_str()),
            ("interrupt-interrupted", interrupted.run_id.as_str()),
        ] {
            persistence
                .interrupts
                .create(
                    interrupt_id,
                    run_id,
                    &thread.id,
                    serde_json::json!({ "kind": "question" }),
                )
                .unwrap();
        }

        let result = persistence.reconcile_after_restart(&thread.id).unwrap();
        assert_eq!(
            result,
            RecoveryReconciliation {
                aborted_runs: 2,
                cancelled_interrupts: 2,
            }
        );
        assert!(persistence
            .runs
            .find_active_run(&thread.id)
            .unwrap()
            .is_none());
        assert!(persistence
            .interrupts
            .list_pending(&thread.id)
            .unwrap()
            .is_empty());
        assert_eq!(
            persistence
                .runs
                .load(&running.run_id)
                .unwrap()
                .unwrap()
                .status,
            RunStatus::Aborted
        );
        assert_eq!(
            persistence
                .runs
                .load(&interrupted.run_id)
                .unwrap()
                .unwrap()
                .status,
            RunStatus::Aborted
        );

        let reconstructed = persistence
            .reconstruct_chat(&thread_store, &thread.id)
            .unwrap();
        assert!(reconstructed.active_run.is_none());
        assert!(reconstructed.pending_interrupts.is_empty());
    }

    #[test]
    fn reconstruct_from_preloaded_thread_does_not_reload_json() {
        let root = scratch("reconstruct-preloaded");
        let thread_store = ThreadStore::open(&root).unwrap();
        let thread = thread_store.create_for_provider("codex").unwrap();
        let persistence = ChatPersistence::open(&root).unwrap();
        persistence
            .runs
            .create_or_resume("run-preloaded", &thread.id)
            .unwrap();

        thread_store.delete(&thread.id).unwrap();
        let reconstructed = persistence
            .reconstruct_chat_from_thread(thread.clone(), None)
            .unwrap();
        assert_eq!(reconstructed.thread.id, thread.id);
        assert_eq!(
            reconstructed
                .active_run
                .as_ref()
                .map(|run| run.run_id.as_str()),
            Some("run-preloaded")
        );
        assert!(thread_store.load(&thread.id).is_err());
    }
}
