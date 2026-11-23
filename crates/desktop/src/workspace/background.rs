//! Toasts for work that happens away from the chat: a background job (dev server, long
//! command) finishing or failing, and a delegation card reaching a state that needs the user
//! (ready to apply, changes requested, blocked, failed, apply conflict). Delegation changes
//! are noticed through the coordinator's notifier even while the board is closed.

use super::*;
use std::collections::HashMap;
use zest_coordinator::DelegationStatus;
use zest_core::jobs::{JobEvent, JobSnapshot, JobStatus};

/// The toast for a job that ended, or `None` for one still running.
pub(super) fn job_alert(job: &JobSnapshot) -> Option<(ToastKind, String)> {
    let detail = job.detail.as_deref().filter(|d| !d.trim().is_empty());
    match job.status {
        JobStatus::Completed => Some((ToastKind::Info, format!("Job finished: {}", job.label))),
        JobStatus::Failed => Some((
            ToastKind::Error,
            match detail {
                Some(detail) => format!("Job failed: {} ({detail})", job.label),
                None => format!("Job failed: {}", job.label),
            },
        )),
        JobStatus::Killed => Some((ToastKind::Info, format!("Job stopped: {}", job.label))),
        JobStatus::Running | JobStatus::Stopping => None,
    }
}

/// What a card moving into `status` should say, if it needs the user.
fn status_message(status: DelegationStatus, title: &str) -> Option<(ToastKind, String)> {
    Some(match status {
        DelegationStatus::ReadyToApply => (ToastKind::Success, format!("Delegation ready to apply: {title}")),
        DelegationStatus::ChangesRequested => (ToastKind::Warning, format!("Reviewer asked for changes: {title}")),
        DelegationStatus::Blocked => (ToastKind::Error, format!("Delegation blocked: {title}")),
        DelegationStatus::Failed => (ToastKind::Error, format!("Delegation failed: {title}")),
        DelegationStatus::ApplyConflict => (ToastKind::Error, format!("Delegation diff did not apply cleanly: {title}")),
        _ => return None,
    })
}

/// The alerts for cards whose status changed since `seen`. Cards seen for the first time
/// alert too (a new card can already be finished), unless `seen` is `None`, which is the
/// first look at the board after the window opened.
pub(super) fn delegation_alerts(
    seen: Option<&HashMap<String, DelegationStatus>>,
    jobs: &[(String, String, DelegationStatus)],
) -> Vec<(ToastKind, String)> {
    let Some(seen) = seen else { return Vec::new() };
    jobs.iter()
        .filter(|(id, _, status)| seen.get(id) != Some(status))
        .filter_map(|(_, title, status)| status_message(*status, title))
        .collect()
}

impl Workspace {
    /// Starts listening for job completions owned by this project.
    pub(super) fn watch_jobs(&mut self, cx: &mut Context<Self>) {
        let mut events = self.project.jobs.subscribe();
        let root = self.project.path.to_string_lossy().to_string();
        cx.spawn(async move |this, cx| {
            loop {
                let job = match events.recv().await {
                    Ok(JobEvent::Completed(job)) => job,
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                if job.owner_root.as_deref().is_some_and(|owner| owner != root) {
                    continue;
                }
                let Some((kind, message)) = job_alert(&job) else { continue };
                if this.update(cx, |ws, cx| ws.toast(kind, message, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Reads the delegation cards and toasts the ones that changed into a state that needs the
    /// user. Runs on every coordinator notification, whether or not the board is open.
    pub(super) fn check_delegations(&mut self, cx: &mut Context<Self>) {
        let root = self.project.path.clone();
        let task = native::executor().spawn_blocking(move || zest_coordinator::list_views(&root));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(views)) = task.await else { return };
            let jobs: Vec<(String, String, DelegationStatus)> =
                views.into_iter().map(|v| (v.job_id, v.title, v.status)).collect();
            let _ = this.update(cx, |ws, cx| {
                for (kind, message) in delegation_alerts(ws.delegation_seen.as_ref(), &jobs) {
                    if matches!(kind, ToastKind::Success) {
                        // Ready to apply waits for the user; do not let it slip away after 3 s.
                        ws.toast_pinned(kind, message, cx);
                    } else {
                        ws.toast(kind, message, cx);
                    }
                }
                ws.delegation_seen = Some(jobs.into_iter().map(|(id, _, status)| (id, status)).collect());
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::{delegation_alerts, job_alert, DelegationStatus, JobSnapshot, ToastKind};
    use std::collections::HashMap;

    fn job(status: &str, detail: Option<&str>) -> JobSnapshot {
        serde_json::from_value(serde_json::json!({
            "id": "job-1", "kind": "command", "label": "npm run dev",
            "status": status, "detail": detail, "startedAt": 1,
        }))
        .unwrap()
    }

    #[::core::prelude::v1::test]
    fn ended_jobs_alert_and_running_ones_do_not() {
        assert_eq!(job_alert(&job("completed", None)), Some((ToastKind::Info, "Job finished: npm run dev".into())));
        assert_eq!(job_alert(&job("failed", Some("exit code 1"))), Some((ToastKind::Error, "Job failed: npm run dev (exit code 1)".into())));
        assert_eq!(job_alert(&job("killed", None)).map(|a| a.0), Some(ToastKind::Info));
        assert_eq!(job_alert(&job("running", None)), None);
    }

    #[::core::prelude::v1::test]
    fn only_changes_into_states_that_need_the_user_alert() {
        let seen = HashMap::from([
            ("a".to_string(), DelegationStatus::WorkerRunning),
            ("b".to_string(), DelegationStatus::ReadyToApply),
            ("c".to_string(), DelegationStatus::ReviewRunning),
        ]);
        let now = [
            ("a".to_string(), "Fix login".to_string(), DelegationStatus::ReadyToApply),
            ("b".to_string(), "Add docs".to_string(), DelegationStatus::ReadyToApply),
            ("c".to_string(), "Port tests".to_string(), DelegationStatus::Cancelled),
            ("d".to_string(), "New card".to_string(), DelegationStatus::Failed),
        ];
        let alerts = delegation_alerts(Some(&seen), &now);
        assert_eq!(
            alerts,
            [
                (ToastKind::Success, "Delegation ready to apply: Fix login".to_string()),
                (ToastKind::Error, "Delegation failed: New card".to_string()),
            ]
        );
    }

    #[::core::prelude::v1::test]
    fn the_first_look_after_opening_is_silent() {
        let now = [("a".to_string(), "Fix login".to_string(), DelegationStatus::Failed)];
        assert!(delegation_alerts(None, &now).is_empty());
    }
}
