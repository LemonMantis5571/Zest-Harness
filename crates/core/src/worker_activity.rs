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
    fn summary_names_the_last_activity() {
        let activity = WorkerActivity::new(WorkerLimits::default());
        assert!(activity.summary("worker").contains("no output yet"));
        activity.progress(Some("tool Read src/lib.rs"));
        let summary = activity.summary("worker");
        assert!(summary.contains("tool Read src/lib.rs"), "{summary}");
        assert!(summary.contains("1 updates"), "{summary}");
    }
}
