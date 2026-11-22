//! What a delegated worker is doing, shared between the process that runs it
//! and the coordinator that watches it.
//!
//! The runner reports progress (any output from an external CLI, any stream
//! event from a native provider) and the coordinator reads it back for the
//! heartbeat. [`watchdog`] turns the same record into the two limits a worker
//! runs under: a total budget and a no-progress budget, so a worker that is
//! stuck waiting on nothing is stopped long before its total budget runs out.

use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

/// Default total budget for a native provider worker.
pub const DEFAULT_WORKER_TOTAL: Duration = Duration::from_secs(60 * 60);
/// Largest transcript kept for one run; later entries are counted, not kept.
pub const MAX_TRANSCRIPT_BYTES: usize = 1024 * 1024;
const MAX_TRANSCRIPT_ENTRY_CHARS: usize = 8_000;

/// Default no-progress budget for a delegated worker or reviewer.
pub const DEFAULT_WORKER_IDLE: Duration = Duration::from_secs(15 * 60);

/// The two limits a delegated run is stopped at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerLimits {
    /// Wall-clock budget for the whole run.
    pub total: Duration,
    /// Longest stretch without any progress.
    pub idle: Duration,
}

impl Default for WorkerLimits {
    fn default() -> Self {
        Self {
            total: DEFAULT_WORKER_TOTAL,
            idle: DEFAULT_WORKER_IDLE,
        }
    }
}

/// Progress record for one worker or reviewer run.
pub struct WorkerActivity {
    limits: WorkerLimits,
    started: Instant,
    state: Mutex<ActivityState>,
}

struct ActivityState {
    last_progress: Instant,
    last_label: Option<String>,
    events: u64,
    partial_diff: Option<String>,
    transcript: Vec<u8>,
    dropped_entries: u64,
    /// Streamed text or thinking still being joined into one entry.
    pending: Option<(&'static str, String, u64)>,
}

impl WorkerActivity {
    pub fn new(limits: WorkerLimits) -> Self {
        let now = Instant::now();
        Self {
            limits,
            started: now,
            state: Mutex::new(ActivityState {
                last_progress: now,
                last_label: None,
                events: 0,
                partial_diff: None,
                transcript: Vec::new(),
                dropped_entries: 0,
                pending: None,
            }),
        }
    }

    pub fn limits(&self) -> WorkerLimits {
        self.limits
    }

    /// The worker showed progress. `label` names what it did, when known.
    pub fn progress(&self, label: Option<&str>) {
        if let Ok(mut state) = self.state.lock() {
            state.last_progress = Instant::now();
            state.events = state.events.saturating_add(1);
            if let Some(label) = label.filter(|label| !label.trim().is_empty()) {
                state.last_label = Some(clip_label(label));
            }
        }
    }

    /// Time since the last progress report.
    pub fn idle_for(&self) -> Duration {
        self.state
            .lock()
            .map(|state| state.last_progress.elapsed())
            .unwrap_or_default()
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Add an entry to the run's transcript. Consecutive `text` or
    /// `thinking` chunks (`streamed`) are joined into one entry.
    pub fn record(&self, kind: &'static str, text: &str, streamed: bool) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let at = self.started.elapsed().as_millis() as u64;
        if streamed {
            match &mut state.pending {
                Some((pending_kind, buffer, _)) if *pending_kind == kind => buffer.push_str(text),
                _ => {
                    flush_pending(&mut state);
                    state.pending = Some((kind, text.to_string(), at));
                }
            }
            return;
        }
        flush_pending(&mut state);
        push_entry(&mut state, kind, text, at);
    }

    /// The transcript as JSON Lines: `{"atMs":…,"kind":…,"text":…}`, with
    /// `atMs` counted from the start of the run.
    pub fn transcript(&self) -> Vec<u8> {
        let Ok(mut state) = self.state.lock() else {
            return Vec::new();
        };
        flush_pending(&mut state);
        let mut out = state.transcript.clone();
        if state.dropped_entries > 0 {
            let line = serde_json::json!({
                "kind": "truncated",
                "text": format!("{} more entries were not kept", state.dropped_entries),
            });
            out.extend_from_slice(line.to_string().as_bytes());
            out.push(b'\n');
        }
        out
    }

    /// Keep what a worker that failed or timed out had changed so far.
    pub fn keep_partial_diff(&self, diff: String) {
        if diff.trim().is_empty() {
            return;
        }
        if let Ok(mut state) = self.state.lock() {
            state.partial_diff = Some(diff);
        }
    }

    /// The diff a failed run left behind, if any.
    pub fn take_partial_diff(&self) -> Option<String> {
        self.state
            .lock()
            .ok()
            .and_then(|mut state| state.partial_diff.take())
    }

    /// One line for the heartbeat: how long the run has gone and what it did
    /// last.
    pub fn summary(&self, role: &str) -> String {
        let Ok(state) = self.state.lock() else {
            return format!("{role} is running");
        };
        let elapsed = self.started.elapsed().as_secs();
        let idle = state.last_progress.elapsed().as_secs();
        if state.events == 0 {
            return format!("{role} running for {elapsed}s; no output yet");
        }
        match &state.last_label {
            Some(label) => format!(
                "{role} running for {elapsed}s; last activity {idle}s ago ({label}); {} updates",
                state.events
            ),
            None => format!(
                "{role} running for {elapsed}s; last activity {idle}s ago; {} updates",
                state.events
            ),
        }
    }
}

fn flush_pending(state: &mut ActivityState) {
    if let Some((kind, text, at)) = state.pending.take() {
        push_entry(state, kind, &text, at);
    }
}

fn push_entry(state: &mut ActivityState, kind: &str, text: &str, at: u64) {
    if text.trim().is_empty() {
        return;
    }
    let text: String = if text.chars().count() > MAX_TRANSCRIPT_ENTRY_CHARS {
        let clipped: String = text.chars().take(MAX_TRANSCRIPT_ENTRY_CHARS - 1).collect();
        format!("{clipped}…")
    } else {
        text.to_string()
    };
    let line = serde_json::json!({ "atMs": at, "kind": kind, "text": text }).to_string();
    if state.transcript.len() + line.len() + 1 > MAX_TRANSCRIPT_BYTES {
        state.dropped_entries = state.dropped_entries.saturating_add(1);
        return;
    }
    state.transcript.extend_from_slice(line.as_bytes());
    state.transcript.push(b'\n');
}

fn clip_label(label: &str) -> String {
    const MAX: usize = 120;
    let label = label.trim();
    if label.chars().count() <= MAX {
        return label.to_string();
    }
    let clipped: String = label.chars().take(MAX - 1).collect();
    format!("{clipped}…")
}

/// Resolve with the reason once `activity` exceeds either of its limits.
/// Race it against the run; it never resolves while the run is healthy.
pub async fn watchdog(activity: &WorkerActivity) -> String {
    let limits = activity.limits();
    loop {
        let elapsed = activity.elapsed();
        if elapsed >= limits.total {
            return format!("timed out after {} seconds", limits.total.as_secs());
        }
        let idle = activity.idle_for();
        if idle >= limits.idle {
            return format!(
                "made no progress for {} seconds and was stopped",
                limits.idle.as_secs()
            );
        }
        let until_total = limits.total - elapsed;
        let until_idle = limits.idle - idle;
        tokio::time::sleep(until_total.min(until_idle).min(Duration::from_secs(1))).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn idle_limit_fires_unless_progress_keeps_arriving() {
        let activity = WorkerActivity::new(WorkerLimits {
            total: Duration::from_secs(600),
            idle: Duration::from_millis(400),
        });
        let watch = watchdog(&activity);
        tokio::pin!(watch);
        for _ in 0..8 {
            tokio::select! {
                reason = &mut watch => panic!("fired while progressing: {reason}"),
                _ = tokio::time::sleep(Duration::from_millis(100)) => activity.progress(Some("tool Read")),
            }
        }
        let reason = watch.await;
        assert!(reason.contains("made no progress"), "{reason}");
        assert!(activity.elapsed() >= Duration::from_millis(1_200));
    }

    #[tokio::test]
    async fn total_limit_fires_even_while_progressing() {
        let activity = WorkerActivity::new(WorkerLimits {
            total: Duration::from_millis(500),
            idle: Duration::from_millis(300),
        });
        let watch = watchdog(&activity);
        tokio::pin!(watch);
        let reason = loop {
            tokio::select! {
                reason = &mut watch => break reason,
                _ = tokio::time::sleep(Duration::from_millis(50)) => activity.progress(None),
            }
        };
        assert!(reason.starts_with("timed out after"), "{reason}");
    }

    #[test]
    fn transcript_joins_streamed_text_and_stays_bounded() {
        let activity = WorkerActivity::new(WorkerLimits::default());
        activity.record("text", "Hel", true);
        activity.record("text", "lo", true);
        activity.record("tool", "Read src/lib.rs", false);
        activity.record("text", "done", true);
        let transcript = String::from_utf8(activity.transcript()).unwrap();
        let entries: Vec<serde_json::Value> = transcript
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let pairs: Vec<(&str, &str)> = entries
            .iter()
            .map(|entry| {
                (
                    entry["kind"].as_str().unwrap(),
                    entry["text"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            pairs,
            [
                ("text", "Hello"),
                ("tool", "Read src/lib.rs"),
                ("text", "done")
            ]
        );

        let big = "x".repeat(MAX_TRANSCRIPT_ENTRY_CHARS);
        for _ in 0..(MAX_TRANSCRIPT_BYTES / MAX_TRANSCRIPT_ENTRY_CHARS + 10) {
            activity.record("tool", &big, false);
        }
        let transcript = activity.transcript();
        assert!(transcript.len() <= MAX_TRANSCRIPT_BYTES + 200);
        assert!(String::from_utf8(transcript)
            .unwrap()
            .contains("\"truncated\""));
    }

    #[test]
    fn summary_names_the_last_activity() {
        let activity = WorkerActivity::new(WorkerLimits::default());
        assert!(activity.summary("worker").contains("no output yet"));
        activity.progress(Some("tool Read src/lib.rs"));
        let summary = activity.summary("worker");
        assert!(summary.contains("tool Read src/lib.rs"), "{summary}");
        assert!(summary.contains("1 updates"), "{summary}");
    }
}
