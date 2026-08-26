//! Durable, provider-neutral orchestration state.
//!
//! This is a projection of coordinator activity, not a replacement for the
//! feature card, delegation job, or canonical chat transcript. It records
//! enough lifecycle and worker evidence for the desktop to explain what is
//! happening across restarts without copying external provider history into a
//! Zest conversation.

use serde::{Deserialize, Serialize};

pub const ORCHESTRATION_FORMAT_VERSION: u32 = 1;
const MAX_INBOX_MESSAGES: usize = 64;
const MAX_LIFECYCLE_ENTRIES: usize = 128;
const MAX_EXTERNAL_SESSION_HISTORY: usize = 32;
const MAX_TEXT_CHARS: usize = 4_000;
const MAX_PREVIEW_CHARS: usize = 8_000;

fn clip(value: impl AsRef<str>, limit: usize) -> String {
    value.as_ref().chars().take(limit).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeLineage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout_path: Option<String>,
    #[serde(default)]
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecyclePhase {
    #[default]
    Planned,
    AwaitingApproval,
    Queued,
    WorkerRunning,
    ReviewRunning,
    ReadyToApply,
    Accepted,
    ChangesRequested,
    Blocked,
    Failed,
    Cancelled,
    ApplyConflict,
}

impl From<crate::delegation::DelegationStatus> for LifecyclePhase {
    fn from(value: crate::delegation::DelegationStatus) -> Self {
        match value {
            crate::delegation::DelegationStatus::Planned => Self::Planned,
            crate::delegation::DelegationStatus::AwaitingApproval => Self::AwaitingApproval,
            crate::delegation::DelegationStatus::Queued => Self::Queued,
            crate::delegation::DelegationStatus::WorkerRunning => Self::WorkerRunning,
            crate::delegation::DelegationStatus::ReviewRunning => Self::ReviewRunning,
            crate::delegation::DelegationStatus::ReadyToApply => Self::ReadyToApply,
            crate::delegation::DelegationStatus::Accepted => Self::Accepted,
            crate::delegation::DelegationStatus::ChangesRequested => Self::ChangesRequested,
            crate::delegation::DelegationStatus::Blocked => Self::Blocked,
            crate::delegation::DelegationStatus::Failed => Self::Failed,
            crate::delegation::DelegationStatus::Cancelled => Self::Cancelled,
            crate::delegation::DelegationStatus::ApplyConflict => Self::ApplyConflict,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchRole {
    Worker,
    Reviewer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchState {
    pub id: String,
    pub role: DispatchRole,
    pub target: String,
    pub attempt: u32,
    pub status: DispatchStatus,
    pub started_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Info,
    Decision,
    Question,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxMessage {
    pub id: String,
    pub kind: MessageKind,
    pub sender: String,
    pub body: String,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    Open,
    Approved,
    Rejected,
    Resolved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionGate {
    pub id: String,
    pub label: String,
    pub status: GateStatus,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub opened_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RetryState {
    pub attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_at: Option<u64>,
}

/// Metadata about a CLI worker session. It is intentionally not a transcript
/// and is never used as the source of truth for the Zest conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalSessionEvidence {
    pub worker_id: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(default)]
    pub resumable: bool,
    pub captured_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleEntry {
    pub phase: LifecyclePhase,
    pub detail: String,
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationState {
    pub version: u32,
    pub run_id: String,
    pub task_id: String,
    pub parent_thread_id: String,
    pub phase: LifecyclePhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch: Option<DispatchState>,
    #[serde(default)]
    pub worktree: WorktreeLineage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat_at: Option<u64>,
    #[serde(default)]
    pub inbox: Vec<InboxMessage>,
    #[serde(default)]
    pub decision_gates: Vec<DecisionGate>,
    #[serde(default)]
    pub retry: RetryState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_session: Option<ExternalSessionEvidence>,
    #[serde(default)]
    pub external_session_history: Vec<ExternalSessionEvidence>,
    #[serde(default)]
    pub lifecycle: Vec<LifecycleEntry>,
}

impl Default for OrchestrationState {
    fn default() -> Self {
        Self {
            version: ORCHESTRATION_FORMAT_VERSION,
            run_id: String::new(),
            task_id: String::new(),
            parent_thread_id: String::new(),
            phase: LifecyclePhase::Planned,
            dispatch: None,
            worktree: WorktreeLineage::default(),
            heartbeat_at: None,
            inbox: Vec::new(),
            decision_gates: Vec::new(),
            retry: RetryState::default(),
            external_session: None,
            external_session_history: Vec::new(),
            lifecycle: Vec::new(),
        }
    }
}

impl OrchestrationState {
    pub fn new(
        run_id: impl Into<String>,
        task_id: impl Into<String>,
        parent_thread_id: impl Into<String>,
        worktree: WorktreeLineage,
    ) -> Self {
        Self {
            version: ORCHESTRATION_FORMAT_VERSION,
            run_id: run_id.into(),
            task_id: task_id.into(),
            parent_thread_id: parent_thread_id.into(),
            worktree,
            ..Self::default()
        }
    }

    pub fn is_uninitialized(&self) -> bool {
        self.run_id.is_empty() || self.task_id.is_empty()
    }

    pub fn set_phase(&mut self, phase: LifecyclePhase, at: u64, detail: impl AsRef<str>) {
        let detail = clip(detail, MAX_TEXT_CHARS);
        self.phase = phase;
        self.push_lifecycle(LifecycleEntry {
            phase,
            detail: detail.clone(),
            at,
            dispatch_id: self.dispatch.as_ref().map(|dispatch| dispatch.id.clone()),
        });

        match phase {
            LifecyclePhase::AwaitingApproval => {
                self.open_gate(
                    "approval",
                    "Approval",
                    "A user must approve this dispatch target",
                    at,
                );
            }
            LifecyclePhase::Queued => {
                self.resolve_gate("approval", GateStatus::Approved, &detail, at);
            }
            LifecyclePhase::ReadyToApply => {
                self.open_gate(
                    "apply",
                    "Apply changes",
                    "Review is complete; applying the worker diff requires confirmation",
                    at,
                );
            }
            LifecyclePhase::Accepted => {
                self.resolve_gate("apply", GateStatus::Approved, &detail, at);
            }
            LifecyclePhase::ChangesRequested => {
                self.open_gate("changes", "Requested changes", &detail, at);
            }
            LifecyclePhase::Blocked | LifecyclePhase::Failed => {
                self.open_gate("recovery", "Recovery", &detail, at);
            }
            LifecyclePhase::Cancelled => {
                for gate in &mut self.decision_gates {
                    if gate.status == GateStatus::Open {
                        gate.status = GateStatus::Rejected;
                        gate.resolved_at = Some(at);
                        gate.detail = Some(detail.clone());
                    }
                }
            }
            LifecyclePhase::Planned
            | LifecyclePhase::WorkerRunning
            | LifecyclePhase::ReviewRunning
            | LifecyclePhase::ApplyConflict => {}
        }
    }

    pub fn start_dispatch(
        &mut self,
        id: impl Into<String>,
        role: DispatchRole,
        target: impl Into<String>,
        attempt: u32,
        at: u64,
    ) {
        let id = id.into();
        let target = target.into();
        let dispatch = DispatchState {
            id: clip(id, MAX_TEXT_CHARS),
            role,
            target: clip(target, MAX_TEXT_CHARS),
            attempt,
            status: DispatchStatus::Running,
            started_at: at,
            heartbeat_at: Some(at),
            finished_at: None,
        };
        self.dispatch = Some(dispatch);
        self.heartbeat_at = Some(at);
        self.push_lifecycle(LifecycleEntry {
            phase: self.phase,
            detail: "dispatch started".into(),
            at,
            dispatch_id: self.dispatch.as_ref().map(|dispatch| dispatch.id.clone()),
        });
    }

    pub fn finish_dispatch(&mut self, status: DispatchStatus, at: u64, detail: impl AsRef<str>) {
        if let Some(dispatch) = &mut self.dispatch {
            dispatch.status = status;
            dispatch.finished_at = Some(at);
            dispatch.heartbeat_at = Some(at);
            self.heartbeat_at = Some(at);
            let dispatch_id = dispatch.id.clone();
            self.push_lifecycle(LifecycleEntry {
                phase: self.phase,
                detail: clip(detail, MAX_TEXT_CHARS),
                at,
                dispatch_id: Some(dispatch_id),
            });
        }
    }

    pub fn heartbeat(&mut self, at: u64, _detail: impl AsRef<str>) {
        self.heartbeat_at = Some(at);
        if let Some(dispatch) = &mut self.dispatch {
            dispatch.heartbeat_at = Some(at);
        }
    }

    pub fn open_gate(
        &mut self,
        id: impl Into<String>,
        label: impl Into<String>,
        detail: impl AsRef<str>,
        at: u64,
    ) {
        let id = clip(id.into(), 200);
        let label = clip(label.into(), 200);
        let detail = clip(detail, MAX_TEXT_CHARS);
        if let Some(gate) = self.decision_gates.iter_mut().find(|gate| gate.id == id) {
            gate.label = label;
            gate.status = GateStatus::Open;
            gate.required = true;
            gate.detail = Some(detail);
            gate.opened_at = at;
            gate.resolved_at = None;
            return;
        }
        self.decision_gates.push(DecisionGate {
            id,
            label,
            status: GateStatus::Open,
            required: true,
            detail: Some(detail),
            opened_at: at,
            resolved_at: None,
        });
    }

    pub fn resolve_gate(
        &mut self,
        id: impl AsRef<str>,
        status: GateStatus,
        detail: impl AsRef<str>,
        at: u64,
    ) {
        if let Some(gate) = self
            .decision_gates
            .iter_mut()
            .find(|gate| gate.id == id.as_ref())
        {
            gate.status = status;
            gate.detail = Some(clip(detail, MAX_TEXT_CHARS));
            gate.resolved_at = Some(at);
        }
    }

    pub fn add_message(
        &mut self,
        kind: MessageKind,
        sender: impl AsRef<str>,
        body: impl AsRef<str>,
        created_at: u64,
    ) {
        let id = format!("message-{created_at}-{}", self.inbox.len());
        if self.inbox.len() >= MAX_INBOX_MESSAGES {
            self.inbox.remove(0);
        }
        self.inbox.push(InboxMessage {
            id,
            kind,
            sender: clip(sender, 200),
            body: clip(body, MAX_TEXT_CHARS),
            created_at,
            read_at: None,
        });
    }

    pub fn record_retry(
        &mut self,
        attempt: u32,
        last_error: Option<impl AsRef<str>>,
        requested_at: u64,
    ) {
        self.retry = RetryState {
            attempt,
            last_error: last_error.map(|error| clip(error, MAX_TEXT_CHARS)),
            next_action: Some("approval required before the next attempt".into()),
            requested_at: Some(requested_at),
        };
    }

    pub fn attach_external_session(&mut self, mut evidence: ExternalSessionEvidence) {
        evidence.worker_id = clip(evidence.worker_id, 200);
        evidence.command = clip(evidence.command, 1_000);
        evidence.model = evidence.model.map(|value| clip(value, 200));
        evidence.session_id = evidence.session_id.map(|value| clip(value, 500));
        evidence.cwd = evidence.cwd.map(|value| clip(value, 2_000));
        evidence.branch = evidence.branch.map(|value| clip(value, 500));
        evidence.preview = evidence.preview.map(|value| clip(value, MAX_PREVIEW_CHARS));
        if self.external_session_history.len() >= MAX_EXTERNAL_SESSION_HISTORY {
            self.external_session_history.remove(0);
        }
        self.external_session_history.push(evidence.clone());
        self.external_session = Some(evidence);
    }

    pub fn clear_external_session(&mut self) {
        self.external_session = None;
        self.external_session_history.clear();
    }

    fn push_lifecycle(&mut self, entry: LifecycleEntry) {
        if self.lifecycle.len() >= MAX_LIFECYCLE_ENTRIES {
            self.lifecycle.remove(0);
        }
        self.lifecycle.push(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_state_tracks_run_task_dispatch_and_lineage() {
        let mut state = OrchestrationState::new(
            "run-1",
            "task-1",
            "thread-1",
            WorktreeLineage {
                base_ref: Some("main".into()),
                start_ref: Some("abc123".into()),
                branch: Some("feature/one".into()),
                checkout_path: Some("C:/worktree/one".into()),
                host: "local".into(),
                parent_task: None,
            },
        );

        assert_eq!(state.run_id, "run-1");
        assert_eq!(state.task_id, "task-1");
        assert_eq!(state.parent_thread_id, "thread-1");
        assert_eq!(state.worktree.branch.as_deref(), Some("feature/one"));

        state.set_phase(LifecyclePhase::Queued, 10, "waiting for a worker lane");
        state.start_dispatch("dispatch-1", DispatchRole::Worker, "claude", 1, 20);
        state.heartbeat(30, "worker is still active");

        assert_eq!(state.phase, LifecyclePhase::Queued);
        assert_eq!(
            state.dispatch.as_ref().map(|item| item.id.as_str()),
            Some("dispatch-1")
        );
        assert_eq!(
            state.dispatch.as_ref().map(|item| item.status),
            Some(DispatchStatus::Running)
        );
        assert_eq!(state.heartbeat_at, Some(30));
        assert_eq!(state.lifecycle.len(), 2);
    }

    #[test]
    fn public_state_keeps_inbox_gates_and_retry_state_bounded() {
        let mut state = OrchestrationState::new("run-1", "task-1", "thread-1", Default::default());
        state.open_gate("approval", "Approval", "A user must approve this task", 1);
        state.resolve_gate(
            "approval",
            GateStatus::Approved,
            "Approved target fingerprint",
            2,
        );
        state.add_message(MessageKind::Decision, "coordinator", "Approval recorded", 3);
        state.record_retry(2, Some("review requested changes"), 4);

        assert_eq!(state.decision_gates[0].status, GateStatus::Approved);
        assert_eq!(state.retry.attempt, 2);
        assert_eq!(
            state.retry.last_error.as_deref(),
            Some("review requested changes")
        );
        assert_eq!(state.inbox.len(), 1);
        assert_eq!(state.inbox[0].kind, MessageKind::Decision);
    }

    #[test]
    fn external_session_evidence_is_separate_from_canonical_history() {
        let mut state = OrchestrationState::new("run-1", "task-1", "thread-1", Default::default());
        state.attach_external_session(ExternalSessionEvidence {
            worker_id: "claude".into(),
            command: "claude".into(),
            model: Some("sonnet".into()),
            session_id: Some("cli-session-1".into()),
            cwd: Some("C:/temp/worktree".into()),
            branch: Some("feature/one".into()),
            preview: Some("Worker summary".into()),
            resumable: true,
            captured_at: 5,
        });

        assert_eq!(
            state
                .external_session
                .as_ref()
                .unwrap()
                .session_id
                .as_deref(),
            Some("cli-session-1")
        );
        assert!(state.external_session.as_ref().unwrap().resumable);
    }
}
