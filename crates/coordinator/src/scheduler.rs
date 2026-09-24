//! Shared coordinator for durable feature-card jobs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

use crate::lock::CoordinatorLock;
use crate::runtime::{
    DelegationNotifier, NoopNotifier, SharedNotifier, SpawnAbort, TaskSpawner, TokioSpawner,
};
#[cfg(feature = "export-bindings")]
use ts_rs::TS;
use zest_core::{
    apply_diff_checked, capture_workspace_snapshot, capture_worktree_lineage, dependency_blocker,
    diff_paths, resolve_provider_target, run_acceptance_checks, run_delegation_reviewer,
    run_delegation_worker, run_provider_reviewer, run_provider_worker, validate_diff_scope,
    AttemptRole, AttemptUsage, CheckStatus, Config, DelegationJob, DelegationOrigin,
    DelegationStatus as CoreDelegationStatus, DelegationStore, DelegationTarget,
    ExternalUsageReport, ProviderConfig, ResolvedTargetMetadata, ReviewReport,
    ReviewSeverity as CoreReviewSeverity, ReviewerTarget, WorkerResult,
};
use zest_core::{
    DecisionGate, DispatchState, ExternalSessionEvidence, InboxMessage, LifecycleEntry,
    OrchestrationState, RetryState, WorktreeLineage,
};

const MAX_ACTIVE_WORKER_JOBS: usize = 2;
const HEARTBEAT_INTERVAL_SECS: u64 = 15;
pub const ARTIFACT_PAGE_BYTES: usize = 64 * 1024;
pub const ALLOWED_ARTIFACTS: &[&str] = &["worker.diff", "worker-result.json", "review-result.json"];
pub const DESKTOP_ORIGIN: &str = "desktop_agent_board";
pub const INBOUND_MCP_ORIGIN: &str = "inbound_mcp";
const SHUTDOWN_WAIT: Duration = Duration::from_secs(8);

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "DelegationStatus.ts", rename_all = "snake_case")
)]
pub enum DelegationStatus {
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

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "ReviewSeverity.ts", rename_all = "snake_case")
)]
pub enum ReviewSeverity {
    Blocking,
    Advisory,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(
        export,
        export_to = "AcceptanceCheckStatus.ts",
        rename_all = "snake_case"
    )
)]
pub enum AcceptanceCheckStatus {
    Pending,
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "ReviewFinding.ts", rename_all = "camelCase")
)]
pub struct ReviewFinding {
    pub severity: ReviewSeverity,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "AcceptanceCheckView.ts", rename_all = "camelCase")
)]
pub struct AcceptanceCheckView {
    pub command: String,
    pub status: AcceptanceCheckStatus,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "DelegationTarget.ts", rename_all = "camelCase")
)]
pub enum DelegationTargetView {
    ExternalAgent {
        #[serde(rename = "agentId")]
        #[cfg_attr(feature = "export-bindings", ts(rename = "agentId"))]
        agent_id: String,
    },
    Provider {
        #[serde(rename = "providerId")]
        #[cfg_attr(feature = "export-bindings", ts(rename = "providerId"))]
        provider_id: String,
        model: Option<String>,
        effort: Option<String>,
    },
}

impl From<&DelegationTarget> for DelegationTargetView {
    fn from(target: &DelegationTarget) -> Self {
        match target {
            DelegationTarget::ExternalAgent { agent_id } => Self::ExternalAgent {
                agent_id: agent_id.clone(),
            },
            DelegationTarget::Provider {
                provider_id,
                model,
                effort,
            } => Self::Provider {
                provider_id: provider_id.clone(),
                model: model.clone(),
                effort: effort.clone(),
            },
        }
    }
}

impl From<DelegationTargetView> for DelegationTarget {
    fn from(target: DelegationTargetView) -> Self {
        match target {
            DelegationTargetView::ExternalAgent { agent_id } => Self::ExternalAgent { agent_id },
            DelegationTargetView::Provider {
                provider_id,
                model,
                effort,
            } => Self::Provider {
                provider_id,
                model,
                effort,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "ReviewerTarget.ts", rename_all = "camelCase")
)]
pub enum ReviewerTargetView {
    SameAsWorker,
    Target { target: DelegationTargetView },
}

impl From<&ReviewerTarget> for ReviewerTargetView {
    fn from(target: &ReviewerTarget) -> Self {
        match target {
            ReviewerTarget::SameAsWorker => Self::SameAsWorker,
            ReviewerTarget::Target(target) => Self::Target {
                target: target.into(),
            },
        }
    }
}

impl From<ReviewerTargetView> for ReviewerTarget {
    fn from(target: ReviewerTargetView) -> Self {
        match target {
            ReviewerTargetView::SameAsWorker => Self::SameAsWorker,
            ReviewerTargetView::Target { target } => Self::Target(target.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(
        export,
        export_to = "DelegationAttemptView.ts",
        rename_all = "camelCase"
    )
)]
pub struct DelegationAttemptView {
    pub attempt_id: String,
    pub role: String,
    pub agent: String,
    pub target: Option<DelegationTargetView>,
    pub usage: Option<AttemptUsageView>,
    pub resolved_target: Option<ResolvedTargetMetadataView>,
    pub started_at: u64,
    pub finished_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
pub struct AttemptUsageView {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(
        export,
        export_to = "ResolvedTargetMetadataView.ts",
        rename_all = "camelCase"
    )
)]
pub struct ResolvedTargetMetadataView {
    pub target: DelegationTargetView,
    pub config_fingerprint: String,
    pub credential_fingerprint: Option<String>,
}

impl From<&ResolvedTargetMetadata> for ResolvedTargetMetadataView {
    fn from(metadata: &ResolvedTargetMetadata) -> Self {
        Self {
            target: (&metadata.target).into(),
            config_fingerprint: metadata.config_fingerprint.clone(),
            credential_fingerprint: metadata.credential_fingerprint.clone(),
        }
    }
}

impl From<&AttemptUsage> for AttemptUsageView {
    fn from(usage: &AttemptUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
pub struct DelegationOriginView {
    pub coordinator: String,
    pub chat_id: Option<String>,
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    pub idempotency_key: Option<String>,
}

impl From<&DelegationOrigin> for DelegationOriginView {
    fn from(origin: &DelegationOrigin) -> Self {
        Self {
            coordinator: origin.coordinator.clone(),
            chat_id: origin.chat_id.clone(),
            thread_id: origin.thread_id.clone(),
            idempotency_key: origin.idempotency_key.clone(),
        }
    }
}

fn serialized_label<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .unwrap_or_default()
        .trim_matches('"')
        .to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "WorktreeLineageView.ts", rename_all = "camelCase")
)]
pub struct WorktreeLineageView {
    pub base_ref: Option<String>,
    pub start_ref: Option<String>,
    pub branch: Option<String>,
    pub checkout_path: Option<String>,
    pub host: String,
    pub parent_task: Option<String>,
}

impl From<&WorktreeLineage> for WorktreeLineageView {
    fn from(lineage: &WorktreeLineage) -> Self {
        Self {
            base_ref: lineage.base_ref.clone(),
            start_ref: lineage.start_ref.clone(),
            branch: lineage.branch.clone(),
            checkout_path: lineage.checkout_path.clone(),
            host: lineage.host.clone(),
            parent_task: lineage.parent_task.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "DispatchView.ts", rename_all = "camelCase")
)]
pub struct DispatchView {
    pub id: String,
    pub role: String,
    pub target: String,
    pub attempt: u32,
    pub status: String,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    pub started_at: u64,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    pub heartbeat_at: Option<u64>,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    pub finished_at: Option<u64>,
}

impl From<&DispatchState> for DispatchView {
    fn from(dispatch: &DispatchState) -> Self {
        Self {
            id: dispatch.id.clone(),
            role: serialized_label(&dispatch.role),
            target: dispatch.target.clone(),
            attempt: dispatch.attempt,
            status: serialized_label(&dispatch.status),
            started_at: dispatch.started_at,
            heartbeat_at: dispatch.heartbeat_at,
            finished_at: dispatch.finished_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "InboxMessageView.ts", rename_all = "camelCase")
)]
pub struct InboxMessageView {
    pub id: String,
    pub kind: String,
    pub sender: String,
    pub body: String,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    pub created_at: u64,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    pub read_at: Option<u64>,
}

impl From<&InboxMessage> for InboxMessageView {
    fn from(message: &InboxMessage) -> Self {
        Self {
            id: message.id.clone(),
            kind: serialized_label(&message.kind),
            sender: message.sender.clone(),
            body: message.body.clone(),
            created_at: message.created_at,
            read_at: message.read_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "DecisionGateView.ts", rename_all = "camelCase")
)]
pub struct DecisionGateView {
    pub id: String,
    pub label: String,
    pub status: String,
    pub required: bool,
    pub detail: Option<String>,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    pub opened_at: u64,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    pub resolved_at: Option<u64>,
}

impl From<&DecisionGate> for DecisionGateView {
    fn from(gate: &DecisionGate) -> Self {
        Self {
            id: gate.id.clone(),
            label: gate.label.clone(),
            status: serialized_label(&gate.status),
            required: gate.required,
            detail: gate.detail.clone(),
            opened_at: gate.opened_at,
            resolved_at: gate.resolved_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "RetryStateView.ts", rename_all = "camelCase")
)]
pub struct RetryStateView {
    pub attempt: u32,
    pub last_error: Option<String>,
    pub next_action: Option<String>,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    pub requested_at: Option<u64>,
}

impl From<&RetryState> for RetryStateView {
    fn from(retry: &RetryState) -> Self {
        Self {
            attempt: retry.attempt,
            last_error: retry.last_error.clone(),
            next_action: retry.next_action.clone(),
            requested_at: retry.requested_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(
        export,
        export_to = "ExternalSessionEvidenceView.ts",
        rename_all = "camelCase"
    )
)]
pub struct ExternalSessionEvidenceView {
    pub worker_id: String,
    pub command: String,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub preview: Option<String>,
    pub resumable: bool,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    pub captured_at: u64,
}

impl From<&ExternalSessionEvidence> for ExternalSessionEvidenceView {
    fn from(evidence: &ExternalSessionEvidence) -> Self {
        Self {
            worker_id: evidence.worker_id.clone(),
            command: evidence.command.clone(),
            model: evidence.model.clone(),
            session_id: evidence.session_id.clone(),
            cwd: evidence.cwd.clone(),
            branch: evidence.branch.clone(),
            preview: evidence.preview.clone(),
            resumable: evidence.resumable,
            captured_at: evidence.captured_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "LifecycleEntryView.ts", rename_all = "camelCase")
)]
pub struct LifecycleEntryView {
    pub phase: String,
    pub detail: String,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    pub at: u64,
    pub dispatch_id: Option<String>,
}

impl From<&LifecycleEntry> for LifecycleEntryView {
    fn from(entry: &LifecycleEntry) -> Self {
        Self {
            phase: serialized_label(&entry.phase),
            detail: entry.detail.clone(),
            at: entry.at,
            dispatch_id: entry.dispatch_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "OrchestrationView.ts", rename_all = "camelCase")
)]
pub struct OrchestrationView {
    pub version: u32,
    pub run_id: String,
    pub task_id: String,
    pub parent_thread_id: String,
    pub phase: String,
    pub dispatch: Option<DispatchView>,
    pub worktree: WorktreeLineageView,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    pub heartbeat_at: Option<u64>,
    pub inbox: Vec<InboxMessageView>,
    pub decision_gates: Vec<DecisionGateView>,
    pub retry: RetryStateView,
    pub external_session: Option<ExternalSessionEvidenceView>,
    pub external_session_history: Vec<ExternalSessionEvidenceView>,
    pub lifecycle: Vec<LifecycleEntryView>,
}

impl From<&OrchestrationState> for OrchestrationView {
    fn from(state: &OrchestrationState) -> Self {
        Self {
            version: state.version,
            run_id: state.run_id.clone(),
            task_id: state.task_id.clone(),
            parent_thread_id: state.parent_thread_id.clone(),
            phase: serialized_label(&state.phase),
            dispatch: state.dispatch.as_ref().map(Into::into),
            worktree: (&state.worktree).into(),
            heartbeat_at: state.heartbeat_at,
            inbox: state.inbox.iter().map(Into::into).collect(),
            decision_gates: state.decision_gates.iter().map(Into::into).collect(),
            retry: (&state.retry).into(),
            external_session: state.external_session.as_ref().map(Into::into),
            external_session_history: state
                .external_session_history
                .iter()
                .map(Into::into)
                .collect(),
            lifecycle: state.lifecycle.iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(
        export,
        export_to = "DelegationCreateRequest.ts",
        rename_all = "camelCase"
    )
)]
pub struct CreateDelegationJobRequest {
    pub parent_thread_id: String,
    pub title: String,
    pub objective: String,
    pub lane: String,
    pub scope: Vec<String>,
    #[serde(default)]
    pub context: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub acceptance_checks: Vec<String>,
    pub worker: DelegationTargetView,
    #[serde(default)]
    pub reviewer: Option<ReviewerTargetView>,
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    pub idempotency_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    pub origin_coordinator: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(
        export,
        export_to = "DelegationUpdateRequest.ts",
        rename_all = "camelCase"
    )
)]
pub struct UpdateDelegationJobRequest {
    pub job_id: String,
    pub worker: Option<DelegationTargetView>,
    pub reviewer: Option<ReviewerTargetView>,
    pub title: Option<String>,
    pub objective: Option<String>,
    pub scope: Option<Vec<String>>,
    pub context: Option<Vec<String>>,
    pub acceptance_checks: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    pub expected_updated_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(
        export,
        export_to = "DelegationTargetOption.ts",
        rename_all = "camelCase"
    )
)]
pub struct DelegationTargetOption {
    pub target: DelegationTargetView,
    pub available: bool,
    pub label: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "DelegationHandoff.ts", rename_all = "camelCase")
)]
pub struct DelegationHandoff {
    pub job_id: String,
    pub summary: String,
    pub changed_files: Vec<String>,
    pub artifact_names: Vec<String>,
    pub status: DelegationStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "DelegationJobView.ts", rename_all = "camelCase")
)]
pub struct DelegationJobView {
    pub job_id: String,
    pub parent_thread_id: String,
    pub project_root: String,
    pub card_id: String,
    pub title: String,
    pub objective: String,
    pub lane: String,
    pub scope: Vec<String>,
    pub context: Vec<String>,
    pub depends_on: Vec<String>,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    pub worker_attempt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    pub reviewer_attempt_id: Option<String>,
    pub reviewer_agent: String,
    pub worker_target: DelegationTargetView,
    pub reviewer_target: ReviewerTargetView,
    pub resolved_worker_target: Option<ResolvedTargetMetadataView>,
    pub resolved_reviewer_target: Option<ResolvedTargetMetadataView>,
    pub approved: bool,
    pub origin: Option<DelegationOriginView>,
    pub attempts: Vec<DelegationAttemptView>,
    pub attempt: u32,
    pub status: DelegationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    pub orchestration: Option<OrchestrationView>,
    pub changed_files: Vec<String>,
    pub changed_file_count: usize,
    pub acceptance_checks: Vec<AcceptanceCheckView>,
    pub reviewer_findings: Vec<ReviewFinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    pub worker_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    pub error: Option<String>,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    pub created_at: u64,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "DelegationEvent.ts", rename_all = "snake_case")
)]
pub enum DelegationEvent {
    CardCreated { job: DelegationJobView },
    ApprovalRequired { job: DelegationJobView },
    Queued { job: DelegationJobView },
    WorkerStarted { job: DelegationJobView },
    Heartbeat { job: DelegationJobView },
    WorkerCompleted { job: DelegationJobView },
    ReviewerStarted { job: DelegationJobView },
    ReviewerCompleted { job: DelegationJobView },
    ChangesRequested { job: DelegationJobView },
    ReadyToApply { job: DelegationJobView },
    Applied { job: DelegationJobView },
    Conflict { job: DelegationJobView },
    Blocked { job: DelegationJobView },
    Failed { job: DelegationJobView },
    Cancelled { job: DelegationJobView },
}

#[derive(Debug, Clone, Copy)]
enum EventKind {
    CardCreated,
    ApprovalRequired,
    Queued,
    WorkerStarted,
    Heartbeat,
    WorkerCompleted,
    ReviewerStarted,
    ReviewerCompleted,
    ChangesRequested,
    ReadyToApply,
    Applied,
    Conflict,
    Blocked,
    Failed,
    Cancelled,
}

impl From<CoreDelegationStatus> for DelegationStatus {
    fn from(status: CoreDelegationStatus) -> Self {
        match status {
            CoreDelegationStatus::Planned => Self::Planned,
            CoreDelegationStatus::AwaitingApproval => Self::AwaitingApproval,
            CoreDelegationStatus::Queued => Self::Queued,
            CoreDelegationStatus::WorkerRunning => Self::WorkerRunning,
            CoreDelegationStatus::ReviewRunning => Self::ReviewRunning,
            CoreDelegationStatus::ReadyToApply => Self::ReadyToApply,
            CoreDelegationStatus::Accepted => Self::Accepted,
            CoreDelegationStatus::ChangesRequested => Self::ChangesRequested,
            CoreDelegationStatus::Blocked => Self::Blocked,
            CoreDelegationStatus::Failed => Self::Failed,
            CoreDelegationStatus::Cancelled => Self::Cancelled,
            CoreDelegationStatus::ApplyConflict => Self::ApplyConflict,
        }
    }
}

impl From<CoreReviewSeverity> for ReviewSeverity {
    fn from(severity: CoreReviewSeverity) -> Self {
        match severity {
            CoreReviewSeverity::Blocking => Self::Blocking,
            CoreReviewSeverity::Advisory => Self::Advisory,
        }
    }
}

impl From<CheckStatus> for AcceptanceCheckStatus {
    fn from(status: CheckStatus) -> Self {
        match status {
            CheckStatus::Passed => Self::Passed,
            CheckStatus::Failed => Self::Failed,
            CheckStatus::Skipped => Self::Skipped,
        }
    }
}

fn report_from_store(store: &DelegationStore, job: &DelegationJob) -> Option<ReviewReport> {
    let bytes = store
        .read_artifact(&job.job_id, "review-result.json")
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn worker_result_from_store(store: &DelegationStore, job: &DelegationJob) -> Option<WorkerResult> {
    let bytes = store
        .read_artifact(&job.job_id, "worker-result.json")
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[derive(Debug, Clone)]
struct ResolvedDelegationTargets {
    worker: ResolvedTargetMetadata,
    reviewer: ResolvedTargetMetadata,
}

fn fingerprint(value: impl AsRef<[u8]>) -> String {
    blake3::hash(value.as_ref()).to_hex().to_string()
}

fn resolve_delegation_target(
    config: &Config,
    registry: &zest_core::ProviderRegistry,
    skipped: &[zest_core::Skipped],
    target: DelegationTarget,
) -> Result<ResolvedTargetMetadata, String> {
    match target {
        DelegationTarget::Provider {
            provider_id,
            model,
            effort,
        } => {
            let resolved = resolve_provider_target(
                registry,
                skipped,
                &provider_id,
                model.as_deref(),
                effort.as_deref(),
            )
            .map_err(|error| error.to_string())?;
            let config_fingerprint = config
                .providers
                .get(&provider_id)
                .map(|entry| fingerprint(format!("provider:{provider_id}:{entry:?}")))
                .unwrap_or_else(|| fingerprint(format!("provider:{provider_id}:missing")));
            // This is deliberately only a readiness/reference fingerprint. The
            // credential value never enters the job record, and keyless local
            // providers do not get a synthetic credential fingerprint.
            let credential_reference = match config.providers.get(&provider_id) {
                Some(ProviderConfig::Anthropic {
                    credential,
                    api_key_env,
                    ..
                }) => credential.as_deref().or(Some(api_key_env.as_str())),
                Some(ProviderConfig::OpenaiCompatible {
                    credential,
                    api_key_env,
                    ..
                }) => credential.as_deref().or(api_key_env.as_deref()),
                _ => None,
            };
            let credential_fingerprint = credential_reference.map(|reference| {
                fingerprint(format!(
                    "provider-credential:{provider_id}:{reference}:ready"
                ))
            });
            Ok(ResolvedTargetMetadata {
                target: DelegationTarget::Provider {
                    provider_id: resolved.provider_id,
                    model: Some(resolved.model),
                    effort: Some(resolved.effort),
                },
                config_fingerprint,
                credential_fingerprint,
            })
        }
        DelegationTarget::ExternalAgent { agent_id } => {
            let agent = config.agents.get(&agent_id).ok_or_else(|| {
                format!(
                    "external worker `{agent_id}` is unavailable. Connect or configure it, or choose another target."
                )
            })?;
            if agent.workspace != zest_core::ExternalWorkspace::Isolated {
                return Err(format!(
                    "external worker `{agent_id}` must use an isolated workspace"
                ));
            }
            Ok(ResolvedTargetMetadata {
                target: DelegationTarget::ExternalAgent {
                    agent_id: agent_id.clone(),
                },
                config_fingerprint: fingerprint(format!("external:{agent_id}:{agent:?}")),
                credential_fingerprint: None,
            })
        }
    }
}

fn resolve_job_targets(
    root: &Path,
    job: &DelegationJob,
) -> Result<ResolvedDelegationTargets, String> {
    let config = Config::find(root).map_err(|error| error.to_string())?;
    let (registry, skipped) = zest_core::ProviderRegistry::from_config_at(&config, root);
    let worker = resolve_delegation_target(&config, &registry, &skipped, job.worker_target())?;
    let reviewer = if matches!(job.reviewer_target, ReviewerTarget::SameAsWorker) {
        worker.clone()
    } else {
        resolve_delegation_target(&config, &registry, &skipped, job.resolved_reviewer_target())?
    };
    Ok(ResolvedDelegationTargets { worker, reviewer })
}

fn attempt_usage_from_external(report: &ExternalUsageReport) -> AttemptUsage {
    AttemptUsage {
        input_tokens: report.input_tokens,
        output_tokens: report.output_tokens,
        cache_read_tokens: report.cached_read_tokens,
        cache_write_tokens: report.cached_write_tokens,
    }
}

fn validate_authoritative_checks(
    expected: &[zest_core::AcceptanceCheckResult],
    report: &ReviewReport,
) -> Result<(), String> {
    for expected_check in expected {
        let Some(actual) = report
            .checks
            .iter()
            .find(|check| check.command == expected_check.command)
        else {
            return Err(format!(
                "reviewer omitted authoritative acceptance check `{}`",
                expected_check.command
            ));
        };
        if actual.status != expected_check.status || actual.output != expected_check.output {
            return Err(format!(
                "reviewer changed authoritative result for `{}`",
                expected_check.command
            ));
        }
    }
    Ok(())
}

pub fn job_view(store: &DelegationStore, job: &DelegationJob) -> DelegationJobView {
    let diff = store
        .read_artifact(&job.job_id, "worker.diff")
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default();
    let changed_files = diff_paths(&diff);
    let report = report_from_store(store, job);
    let acceptance_checks = job
        .card
        .acceptance_checks
        .iter()
        .map(|command| {
            report
                .as_ref()
                .and_then(|report| report.checks.iter().find(|check| check.command == *command))
                .map(|check| AcceptanceCheckView {
                    command: check.command.clone(),
                    status: check.status.into(),
                    output: check.output.clone(),
                })
                .unwrap_or_else(|| AcceptanceCheckView {
                    command: command.clone(),
                    status: AcceptanceCheckStatus::Pending,
                    output: String::new(),
                })
        })
        .collect::<Vec<_>>();
    let reviewer_findings = report
        .map(|report| {
            report
                .findings
                .into_iter()
                .map(|finding| ReviewFinding {
                    severity: finding.severity.into(),
                    path: finding.path,
                    message: finding.message,
                })
                .collect()
        })
        .unwrap_or_default();
    let worker_summary = worker_result_from_store(store, job).map(|result| result.summary);
    DelegationJobView {
        job_id: job.job_id.clone(),
        parent_thread_id: job.parent_thread_id.clone(),
        project_root: job.project_root.clone(),
        card_id: job.card.card_id.clone(),
        title: job.card.title.clone(),
        objective: job.card.objective.clone(),
        lane: job.card.lane.clone(),
        scope: job.card.scope.clone(),
        context: job.card.context.clone(),
        depends_on: job.card.depends_on.clone(),
        agent: job.worker_target().identity().to_string(),
        worker_attempt_id: job.worker_attempt_id.clone(),
        reviewer_attempt_id: job.reviewer_attempt_id.clone(),
        reviewer_agent: job.resolved_reviewer_target().identity().to_string(),
        worker_target: (&job.worker_target()).into(),
        reviewer_target: (&job.reviewer_target).into(),
        resolved_worker_target: job.resolved_worker_target.as_ref().map(Into::into),
        resolved_reviewer_target: job.resolved_reviewer_target.as_ref().map(Into::into),
        approved: job.is_approved(),
        origin: job.origin.as_ref().map(Into::into),
        attempts: job
            .attempts
            .iter()
            .map(|attempt| DelegationAttemptView {
                attempt_id: attempt.attempt_id.clone(),
                role: format!("{:?}", attempt.role).to_lowercase(),
                agent: attempt.agent.clone(),
                target: attempt.target.as_ref().map(Into::into),
                usage: attempt.usage.as_ref().map(Into::into),
                resolved_target: attempt.resolved_target.as_ref().map(Into::into),
                started_at: attempt.started_at,
                finished_at: attempt.finished_at,
            })
            .collect(),
        attempt: job.attempt,
        status: job.status.into(),
        orchestration: Some((&job.orchestration).into()),
        changed_file_count: changed_files.len(),
        changed_files,
        acceptance_checks,
        reviewer_findings,
        worker_summary,
        error: job.error.clone(),
        created_at: job.created_at,
        updated_at: job.updated_at,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactPage {
    pub name: String,
    pub offset: u64,
    pub content: String,
    pub total_bytes: u64,
    pub next_offset: Option<u64>,
}

pub struct DelegationCoordinator {
    lanes: Arc<Semaphore>,
    running: Mutex<HashMap<String, Arc<zest_core::CancelToken>>>,
    ledger: Arc<Mutex<zest_core::Ledger>>,
    notifier: SharedNotifier,
    spawner: Arc<dyn TaskSpawner>,
    locks: Mutex<HashMap<PathBuf, CoordinatorLock>>,
    ops: Mutex<()>,
    shutting_down: AtomicBool,
}

impl DelegationCoordinator {
    pub fn new() -> Self {
        Self::with_ledger(Arc::new(Mutex::new(zest_core::Ledger::load())))
    }

    pub fn with_ledger(ledger: Arc<Mutex<zest_core::Ledger>>) -> Self {
        Self::with_runtime(
            ledger,
            Arc::new(TokioSpawner::current()),
            Arc::new(NoopNotifier),
        )
    }

    pub fn with_runtime(
        ledger: Arc<Mutex<zest_core::Ledger>>,
        spawner: Arc<dyn TaskSpawner>,
        notifier: Arc<dyn DelegationNotifier>,
    ) -> Self {
        Self {
            lanes: Arc::new(Semaphore::new(MAX_ACTIVE_WORKER_JOBS)),
            running: Mutex::new(HashMap::new()),
            ledger,
            notifier: SharedNotifier::new(notifier),
            spawner,
            locks: Mutex::new(HashMap::new()),
            ops: Mutex::new(()),
            shutting_down: AtomicBool::new(false),
        }
    }

    fn lock_ops(&self) -> Result<MutexGuard<'_, ()>, String> {
        self.ops
            .lock()
            .map_err(|_| "coordinator ops lock poisoned".to_string())
    }

    pub fn set_notifier(&self, notifier: Arc<dyn DelegationNotifier>) {
        self.notifier.set(notifier);
    }

    pub fn ensure_lock(&self, root: &Path) -> Result<(), String> {
        let key = canonicalize_root(root)?;
        {
            let locks = self
                .locks
                .lock()
                .map_err(|_| "coordinator lock map is poisoned".to_string())?;
            if locks.contains_key(&key) {
                return Ok(());
            }
        }
        match CoordinatorLock::acquire(&key) {
            Ok(held) => {
                let mut locks = self
                    .locks
                    .lock()
                    .map_err(|_| "coordinator lock map is poisoned".to_string())?;
                locks.entry(key).or_insert(held);
                Ok(())
            }
            Err(error) => {
                let locks = self
                    .locks
                    .lock()
                    .map_err(|_| "coordinator lock map is poisoned".to_string())?;
                if locks.contains_key(&key) {
                    Ok(())
                } else {
                    Err(error)
                }
            }
        }
    }

    fn require_lock(&self, root: &Path) -> Result<(), String> {
        self.ensure_lock(root)
    }

    fn record_heartbeat(&self, root: &Path, job_id: &str, detail: &str) -> bool {
        let Ok(store) = DelegationStore::open(root) else {
            return false;
        };
        let Ok(Some(mut job)) = store.load(job_id) else {
            return false;
        };
        if !matches!(
            job.status,
            CoreDelegationStatus::WorkerRunning | CoreDelegationStatus::ReviewRunning
        ) {
            return false;
        }
        job.orchestration.heartbeat(unix_millis(), detail);
        if store.update(job.clone()).is_err() {
            return false;
        }
        self.emit(&store, &job, EventKind::Heartbeat);
        true
    }

    fn start_heartbeat_loop(
        self: &Arc<Self>,
        root: PathBuf,
        job_id: String,
        cancel: zest_core::CancelToken,
        detail: String,
    ) -> Box<dyn SpawnAbort> {
        let coordinator = self.clone();
        self.spawner.spawn(Box::pin(async move {
            let mut ticker =
                tokio::time::interval(std::time::Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        if !coordinator.record_heartbeat(&root, &job_id, &detail) {
                            break;
                        }
                    }
                    _ = cancel.cancelled() => break,
                }
            }
        }))
    }

    pub fn list_targets(root: &Path) -> Result<Vec<DelegationTargetOption>, String> {
        let config = Config::find(root).map_err(|error| error.to_string())?;
        let (registry, skipped) = zest_core::ProviderRegistry::from_config_at(&config, root);
        let mut targets = Vec::new();
        for provider_id in config.providers.keys() {
            let target = DelegationTarget::Provider {
                provider_id: provider_id.clone(),
                model: None,
                effort: None,
            };
            let error = resolve_provider_target(&registry, &skipped, provider_id, None, None)
                .err()
                .map(|error| error.to_string());
            targets.push(DelegationTargetOption {
                label: provider_id.clone(),
                available: error.is_none(),
                target: (&target).into(),
                error,
            });
        }
        for (agent_id, agent) in &config.agents {
            let target = DelegationTarget::ExternalAgent {
                agent_id: agent_id.clone(),
            };
            let error = if agent.workspace != zest_core::ExternalWorkspace::Isolated {
                Some("external delegation requires an isolated workspace".into())
            } else {
                None
            };
            targets.push(DelegationTargetOption {
                label: agent_id.clone(),
                available: error.is_none(),
                target: (&target).into(),
                error,
            });
        }
        Ok(targets)
    }

    pub fn create_job(&self, root: &Path, request: CreateDelegationJobRequest) -> ResultView {
        self.require_lock(root)?;
        let _ops = self.lock_ops()?;
        let origin_coordinator = request
            .origin_coordinator
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DESKTOP_ORIGIN)
            .to_string();
        let idempotency_key = request
            .idempotency_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if origin_coordinator != DESKTOP_ORIGIN && idempotency_key.is_none() {
            return Err("delegation_create requires idempotencyKey".into());
        }
        let store = DelegationStore::open(root).map_err(|error| error.to_string())?;
        if let Some(key) = idempotency_key.as_deref() {
            if let Some(existing) = store
                .find_by_idempotency_key(key)
                .map_err(|error| error.to_string())?
            {
                return Ok(job_view(&store, &existing));
            }
        }
        let worker: DelegationTarget = request.worker.into();
        let reviewer: ReviewerTarget = request.reviewer.map(Into::into).unwrap_or_default();
        let card = zest_core::FeatureCard {
            version: zest_core::DELEGATION_FORMAT_VERSION,
            card_id: String::new(),
            title: request.title,
            objective: request.objective,
            lane: request.lane,
            scope: request.scope,
            context: request.context,
            depends_on: request.depends_on,
            agent: worker.identity().to_string(),
            worker_target: Some(worker),
            acceptance_checks: request.acceptance_checks,
            review_required: true,
            reviewer_target: reviewer.clone(),
            created_at: 0,
        };
        let mut job = store
            .create(
                &request.parent_thread_id,
                card,
                capture_workspace_snapshot(root),
            )
            .map_err(|error| error.to_string())?;
        job.reviewer_target = reviewer;
        job.origin = Some(DelegationOrigin {
            coordinator: origin_coordinator,
            chat_id: request.chat_id,
            thread_id: Some(request.parent_thread_id),
            idempotency_key: idempotency_key.clone(),
        });
        job.idempotency_key = idempotency_key;
        let job = store.update(job).map_err(|error| error.to_string())?;
        Ok(job_view(&store, &job))
    }

    pub fn approve(
        self: &Arc<Self>,
        root: &Path,
        job_id: &str,
        expected_updated_at: Option<u64>,
    ) -> ResultView {
        self.require_lock(root)?;
        let _ops = self.lock_ops()?;
        self.approve_inner(root, job_id, expected_updated_at)
    }

    fn approve_inner(
        self: &Arc<Self>,
        root: &Path,
        job_id: &str,
        expected_updated_at: Option<u64>,
    ) -> ResultView {
        let store = DelegationStore::open(root).map_err(|error| error.to_string())?;
        let mut job = store
            .load(job_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "delegation job was not found".to_string())?;
        let already_approved = job.is_approved()
            && matches!(
                job.status,
                CoreDelegationStatus::Queued
                    | CoreDelegationStatus::WorkerRunning
                    | CoreDelegationStatus::ReviewRunning
                    | CoreDelegationStatus::ReadyToApply
                    | CoreDelegationStatus::Accepted
            );
        if already_approved {
            return Ok(job_view(&store, &job));
        }
        job.require_updated_at(expected_updated_at)
            .map_err(|error| error.to_string())?;
        let resolved = match resolve_job_targets(root, &job) {
            Ok(resolved) => resolved,
            Err(error) => {
                job.transition(CoreDelegationStatus::Blocked)
                    .map_err(|e| e.to_string())?;
                job.set_error(error.clone());
                let job = store.update(job).map_err(|e| e.to_string())?;
                self.emit(&store, &job, EventKind::Blocked);
                return Ok(job_view(&store, &job));
            }
        };
        job.approve_with_resolved_targets(resolved.worker, resolved.reviewer)
            .map_err(|error| error.to_string())?;
        let snapshot = capture_workspace_snapshot(root);
        job.base_workspace_snapshot = snapshot.clone();
        job.orchestration.worktree = capture_worktree_lineage(root, &snapshot);
        job.transition(CoreDelegationStatus::Queued)
            .map_err(|error| error.to_string())?;
        let job = store.update(job).map_err(|error| error.to_string())?;
        self.emit(&store, &job, EventKind::Queued);
        self.enqueue(root.to_path_buf(), job.job_id.clone());
        Ok(job_view(&store, &job))
    }

    pub fn apply_dispatch_receipt(self: &Arc<Self>, root: &Path, job_id: &str) -> ResultView {
        self.require_lock(root)?;
        let _ops = self.lock_ops()?;
        let store = DelegationStore::open(root).map_err(|error| error.to_string())?;
        let job = store
            .load(job_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "delegation job was not found".to_string())?;
        if job.is_approved()
            && job.status != CoreDelegationStatus::AwaitingApproval
            && job.status != CoreDelegationStatus::Blocked
            && job.status != CoreDelegationStatus::ChangesRequested
        {
            return Ok(job_view(&store, &job));
        }
        if !job.has_valid_dispatch_receipt() {
            return Err(
                "this card has no recorded dispatch approval for the current target".into(),
            );
        }
        self.approve_inner(root, job_id, None)
    }

    pub fn ingest_dispatch_receipts(
        self: &Arc<Self>,
        root: &Path,
    ) -> Result<Vec<DelegationJobView>, String> {
        self.require_lock(root)?;
        let _ops = self.lock_ops()?;
        self.ingest_inner(root)
    }

    fn ingest_inner(self: &Arc<Self>, root: &Path) -> Result<Vec<DelegationJobView>, String> {
        let store = DelegationStore::open(root).map_err(|error| error.to_string())?;
        let mut ingested = Vec::new();
        for job in store.list().map_err(|error| error.to_string())? {
            if job.status != CoreDelegationStatus::AwaitingApproval {
                continue;
            }
            if !job.has_valid_dispatch_receipt() {
                continue;
            }
            ingested.push(self.approve_inner(root, &job.job_id, None)?);
        }
        Ok(ingested)
    }

    pub fn update_job(&self, root: &Path, request: UpdateDelegationJobRequest) -> ResultView {
        self.require_lock(root)?;
        let _ops = self.lock_ops()?;
        let store = DelegationStore::open(root).map_err(|error| error.to_string())?;
        let mut job = store
            .load(&request.job_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "delegation job was not found".to_string())?;
        job.require_updated_at(request.expected_updated_at)
            .map_err(|error| error.to_string())?;
        if !matches!(
            job.status,
            CoreDelegationStatus::AwaitingApproval
                | CoreDelegationStatus::Blocked
                | CoreDelegationStatus::ChangesRequested
        ) {
            return Err("only jobs awaiting approval can be edited".into());
        }
        if let Some(target) = request.worker {
            job.card.worker_target = Some(target.into());
        }
        if let Some(target) = request.reviewer {
            job.reviewer_target = target.into();
        }
        job.card.reviewer_target = job.reviewer_target.clone();
        if let Some(title) = request.title {
            job.card.title = title;
        }
        if let Some(objective) = request.objective {
            job.card.objective = objective;
        }
        if let Some(scope) = request.scope {
            job.card.scope = scope;
        }
        if let Some(context) = request.context {
            job.card.context = context;
        }
        if let Some(checks) = request.acceptance_checks {
            job.card.acceptance_checks = checks;
        }
        job.card.agent = job.worker_target().identity().to_string();
        // Any edit invalidates the resolved target/configuration that was
        // previously approved, even when the job is already AwaitingApproval
        // and therefore has no status transition to clear it for us.
        job.approved_at = None;
        job.approval_fingerprint = None;
        job.resolved_worker_target = None;
        job.resolved_reviewer_target = None;
        if job.status != CoreDelegationStatus::AwaitingApproval {
            job.transition(CoreDelegationStatus::AwaitingApproval)
                .map_err(|e| e.to_string())?;
        }
        let job = store.update(job).map_err(|error| error.to_string())?;
        Ok(job_view(&store, &job))
    }

    pub fn handoff(&self, root: &Path, job_id: &str) -> Result<DelegationHandoff, String> {
        let store = DelegationStore::open(root).map_err(|e| e.to_string())?;
        let job = store
            .load(job_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "delegation job was not found".to_string())?;
        let summary = worker_result_from_store(&store, &job)
            .map(|r| r.summary)
            .unwrap_or_else(|| {
                job.error
                    .clone()
                    .unwrap_or_else(|| "No worker summary is available yet.".into())
            });
        let diff = store
            .read_artifact(job_id, "worker.diff")
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default();
        Ok(DelegationHandoff {
            job_id: job.job_id,
            summary: summary.chars().take(8_000).collect(),
            changed_files: diff_paths(&diff),
            artifact_names: vec![
                "worker.diff".into(),
                "worker-result.json".into(),
                "review-result.json".into(),
            ],
            status: job.status.into(),
        })
    }

    pub fn enqueue(self: &Arc<Self>, root: PathBuf, job_id: String) {
        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        let cancel = Arc::new(zest_core::CancelToken::new());
        let inserted = self
            .running
            .lock()
            .map(|mut running| {
                if running.contains_key(&job_id) {
                    false
                } else {
                    running.insert(job_id.clone(), cancel.clone());
                    true
                }
            })
            .unwrap_or(false);
        if !inserted {
            return;
        }
        if let Ok(store) = DelegationStore::open(&root) {
            if let Ok(Some(job)) = store.load(&job_id) {
                self.emit(&store, &job, EventKind::CardCreated);
            }
        }
        let coordinator = self.clone();
        self.spawner.spawn(Box::pin(async move {
            let permit = coordinator.lanes.clone().acquire_owned().await;
            let result = match permit {
                Ok(_permit) => coordinator.run_job(&root, &job_id, cancel.as_ref()).await,
                Err(error) => Err(format!("delegation scheduler stopped: {error}")),
            };
            if let Err(error) = result {
                let kind = if error.contains("unavailable")
                    || error.contains("not configured")
                    || error.contains("owns its agent loop")
                    || error.contains("Connect")
                {
                    EventKind::Blocked
                } else {
                    EventKind::Failed
                };
                let _ = coordinator.fail(&root, &job_id, &error, kind);
            }
            if let Ok(mut running) = coordinator.running.lock() {
                if running
                    .get(&job_id)
                    .is_some_and(|current| Arc::ptr_eq(current, &cancel))
                {
                    running.remove(&job_id);
                }
            }
        }));
    }

    pub fn cancel(
        self: &Arc<Self>,
        root: &Path,
        job_id: &str,
        expected_updated_at: Option<u64>,
    ) -> ResultView {
        self.require_lock(root)?;
        let _ops = self.lock_ops()?;
        if let Ok(running) = self.running.lock() {
            if let Some(cancel) = running.get(job_id) {
                cancel.cancel();
            }
        }
        let store = match DelegationStore::open(root) {
            Ok(store) => store,
            Err(error) => return Err(error.to_string()),
        };
        let mut job = match store.load(job_id) {
            Ok(Some(job)) => job,
            Ok(None) => return Err("delegation job was not found".into()),
            Err(error) => return Err(error.to_string()),
        };
        if job.status == CoreDelegationStatus::Cancelled {
            return Ok(job_view(&store, &job));
        }
        if let Err(error) = job.require_updated_at(expected_updated_at) {
            return Err(error.to_string());
        }
        if !job.status.is_terminal() {
            if let Err(error) = job.transition(CoreDelegationStatus::Cancelled) {
                return Err(error.to_string());
            }
            let job = match store.update(job) {
                Ok(job) => job,
                Err(error) => return Err(error.to_string()),
            };
            self.emit(&store, &job, EventKind::Cancelled);
            self.kick_pending(root, &store);
            return Ok(job_view(&store, &job));
        }
        Ok(job_view(&store, &job))
    }

    pub fn retry(
        self: &Arc<Self>,
        root: &Path,
        job_id: &str,
        expected_updated_at: Option<u64>,
    ) -> ResultView {
        self.require_lock(root)?;
        let _ops = self.lock_ops()?;
        let store = DelegationStore::open(root).map_err(|error| error.to_string())?;
        let mut job = store
            .load(job_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "delegation job was not found".to_string())?;
        job.require_updated_at(expected_updated_at)
            .map_err(|error| error.to_string())?;
        if matches!(
            job.status,
            CoreDelegationStatus::ReadyToApply | CoreDelegationStatus::Accepted
        ) {
            return Err(
                "accepted changes must be applied or left untouched before retrying".into(),
            );
        }
        if job.status != CoreDelegationStatus::AwaitingApproval {
            job.transition(CoreDelegationStatus::AwaitingApproval)
                .map_err(|error| error.to_string())?;
        }
        let retry_at = unix_millis();
        let previous_error = job.error.clone();
        job.orchestration.record_retry(
            job.attempt.saturating_add(1),
            previous_error.as_deref(),
            retry_at,
        );
        job.orchestration.add_message(
            zest_core::MessageKind::Decision,
            "coordinator",
            "Retry requested; approval is required before dispatch",
            retry_at,
        );
        let snapshot = capture_workspace_snapshot(root);
        job.base_workspace_snapshot = snapshot.clone();
        job.orchestration.worktree = capture_worktree_lineage(root, &snapshot);
        job.error = None;
        let job = store.update(job).map_err(|error| error.to_string())?;
        self.emit(&store, &job, EventKind::ApprovalRequired);
        Ok(job_view(&store, &job))
    }

    pub fn reconcile(self: &Arc<Self>, root: &Path) -> Result<Vec<DelegationJobView>, String> {
        self.require_lock(root)?;
        let _ops = self.lock_ops()?;
        let _ = self.ingest_inner(root)?;
        let store = DelegationStore::open(root).map_err(|error| error.to_string())?;
        let live_jobs = self
            .running
            .lock()
            .map(|running| {
                running
                    .keys()
                    .cloned()
                    .collect::<std::collections::HashSet<_>>()
            })
            .unwrap_or_default();
        let mut changed = Vec::new();
        for mut job in store.list().map_err(|error| error.to_string())? {
            let interrupted = matches!(
                job.status,
                CoreDelegationStatus::WorkerRunning | CoreDelegationStatus::ReviewRunning
            ) && !live_jobs.contains(&job.job_id);
            if !interrupted {
                continue;
            }
            job.transition(CoreDelegationStatus::Blocked)
                .map_err(|error| error.to_string())?;
            job.finish_active_attempts();
            job.set_error("external delegation process was interrupted; review and retry");
            store
                .update(job.clone())
                .map_err(|error| error.to_string())?;
            changed.push(job);
        }
        let views = changed
            .iter()
            .map(|job| {
                self.emit(&store, job, EventKind::Blocked);
                job_view(&store, job)
            })
            .collect();
        self.kick_pending(root, &store);
        Ok(views)
    }

    pub fn apply(
        self: &Arc<Self>,
        root: &Path,
        job_id: &str,
        expected_updated_at: Option<u64>,
    ) -> ResultView {
        self.require_lock(root)?;
        let _ops = self.lock_ops()?;
        self.apply_inner(root, job_id, expected_updated_at)
    }

    pub fn apply_ready_jobs(
        self: &Arc<Self>,
        root: &Path,
    ) -> Result<Vec<DelegationJobView>, String> {
        self.require_lock(root)?;
        let _ops = self.lock_ops()?;
        let store = DelegationStore::open(root).map_err(|error| error.to_string())?;
        let ready: Vec<String> = store
            .list()
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|job| job.status == CoreDelegationStatus::ReadyToApply)
            .map(|job| job.job_id)
            .collect();
        let mut applied = Vec::with_capacity(ready.len());
        for job_id in ready {
            applied.push(self.apply_inner(root, &job_id, None)?);
        }
        Ok(applied)
    }

    fn apply_inner(
        self: &Arc<Self>,
        root: &Path,
        job_id: &str,
        expected_updated_at: Option<u64>,
    ) -> ResultView {
        let store = DelegationStore::open(root).map_err(|error| error.to_string())?;
        let mut job = store
            .load(job_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "delegation job was not found".to_string())?;
        if job.status == CoreDelegationStatus::Accepted {
            return Ok(job_view(&store, &job));
        }
        if job.status != CoreDelegationStatus::ReadyToApply {
            return Err("only reviewed changes are ready to apply".into());
        }
        job.require_updated_at(expected_updated_at)
            .map_err(|error| error.to_string())?;
        let diff = String::from_utf8(
            store
                .read_artifact(job_id, "worker.diff")
                .map_err(|error| error.to_string())?,
        )
        .map_err(|_| "worker diff is not valid UTF-8".to_string())?;
        if let Err(error) = validate_diff_scope(root, &diff, &job.card.scope)
            .and_then(|()| apply_diff_checked(root, &diff))
        {
            let _ = job.transition(CoreDelegationStatus::ApplyConflict);
            job.set_error(error.to_string());
            let job = store.update(job).map_err(|save| save.to_string())?;
            self.emit(&store, &job, EventKind::Conflict);
            return Ok(job_view(&store, &job));
        }
        job.transition(CoreDelegationStatus::Accepted)
            .map_err(|error| error.to_string())?;
        job.error = None;
        let job = store.update(job).map_err(|error| error.to_string())?;
        self.emit(&store, &job, EventKind::Applied);
        self.kick_pending(root, &store);
        Ok(job_view(&store, &job))
    }

    pub fn artifact_page(
        &self,
        root: &Path,
        job_id: &str,
        name: &str,
        offset: u64,
    ) -> Result<ArtifactPage, String> {
        if !ALLOWED_ARTIFACTS.contains(&name) {
            return Err(format!("artifact `{name}` is not readable"));
        }
        let store = DelegationStore::open(root).map_err(|error| error.to_string())?;
        let bytes = store
            .read_artifact(job_id, name)
            .map_err(|error| error.to_string())?;
        let total = bytes.len() as u64;
        let start = offset.min(total) as usize;
        let end = (start + ARTIFACT_PAGE_BYTES).min(bytes.len());
        let slice = &bytes[start..end];
        let content = String::from_utf8_lossy(slice).into_owned();
        let next_offset = (end as u64 != total).then_some(end as u64);
        Ok(ArtifactPage {
            name: name.to_string(),
            offset,
            content,
            total_bytes: total,
            next_offset,
        })
    }

    pub async fn shutdown(self: &Arc<Self>, root: &Path) {
        self.shutting_down.store(true, Ordering::SeqCst);
        if let Ok(running) = self.running.lock() {
            for token in running.values() {
                token.cancel();
            }
        }
        let deadline = tokio::time::Instant::now() + SHUTDOWN_WAIT;
        while tokio::time::Instant::now() < deadline {
            let empty = self
                .running
                .lock()
                .map(|running| running.is_empty())
                .unwrap_or(true);
            if empty {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let _ = self.mark_interrupted_blocked(root);
    }

    fn mark_interrupted_blocked(&self, root: &Path) -> Result<Vec<DelegationJobView>, String> {
        let _ops = self.lock_ops()?;
        let store = DelegationStore::open(root).map_err(|error| error.to_string())?;
        let live_jobs = self
            .running
            .lock()
            .map(|running| {
                running
                    .keys()
                    .cloned()
                    .collect::<std::collections::HashSet<_>>()
            })
            .unwrap_or_default();
        let mut views = Vec::new();
        for mut job in store.list().map_err(|error| error.to_string())? {
            let interrupted = matches!(
                job.status,
                CoreDelegationStatus::WorkerRunning | CoreDelegationStatus::ReviewRunning
            ) && !live_jobs.contains(&job.job_id);
            if !interrupted {
                continue;
            }
            if job.transition(CoreDelegationStatus::Blocked).is_err() {
                continue;
            }
            job.finish_active_attempts();
            job.set_error("the coordinator stopped while this job was running; review and retry");
            if store.update(job.clone()).is_ok() {
                self.emit(&store, &job, EventKind::Blocked);
                views.push(job_view(&store, &job));
            }
        }
        Ok(views)
    }

    fn kick_pending(self: &Arc<Self>, root: &Path, store: &DelegationStore) {
        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        let Ok(jobs) = store.list() else { return };
        for candidate in &jobs {
            if candidate.status != CoreDelegationStatus::Queued || !candidate.is_approved() {
                continue;
            }
            if let Some(reason) = dependency_blocker(candidate, &jobs) {
                let mut blocked = candidate.clone();
                if blocked.transition(CoreDelegationStatus::Blocked).is_err() {
                    continue;
                }
                blocked.set_error(reason);
                if store.update(blocked.clone()).is_ok() {
                    self.emit(store, &blocked, EventKind::Blocked);
                }
                continue;
            }
            let ready = candidate.card.depends_on.iter().all(|dependency| {
                jobs.iter()
                    .find(|other| &other.job_id == dependency)
                    .is_some_and(|other| other.status == CoreDelegationStatus::Accepted)
            });
            if ready {
                // This is only a queue wake-up for a card whose initial
                // approval already happened; a fresh fix still comes through
                // `retry`, which is the explicit approval action.
                self.enqueue(root.to_path_buf(), candidate.job_id.clone());
            }
        }
    }

    async fn run_job(
        self: &Arc<Self>,
        root: &Path,
        job_id: &str,
        cancel: &zest_core::CancelToken,
    ) -> Result<(), String> {
        let store = DelegationStore::open(root).map_err(|error| error.to_string())?;
        let mut job = store
            .load(job_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "delegation job disappeared".to_string())?;
        if job.status != CoreDelegationStatus::Queued || !job.is_approved() {
            return Ok(());
        }
        let dependencies = store.list().map_err(|error| error.to_string())?;
        if let Some(reason) = dependency_blocker(&job, &dependencies) {
            job.transition(CoreDelegationStatus::Blocked)
                .map_err(|error| error.to_string())?;
            job.set_error(reason);
            store
                .update(job.clone())
                .map_err(|error| error.to_string())?;
            self.emit(&store, &job, EventKind::Blocked);
            return Ok(());
        }
        if job.card.depends_on.iter().any(|dependency| {
            dependencies
                .iter()
                .find(|candidate| &candidate.job_id == dependency)
                .is_none_or(|candidate| candidate.status != CoreDelegationStatus::Accepted)
        }) {
            return Ok(());
        }
        if cancel.is_cancelled() {
            return self.cancelled(&store, job).await;
        }
        let config = Config::find(root).map_err(|error| error.to_string())?;
        let parent_secret_envs = config.provider_key_env_names();
        let resolved_targets = resolve_job_targets(root, &job).map_err(|error| {
            format!("{error}. Reconnect the provider/worker or choose another target.")
        })?;
        if !job.is_approved_for(&resolved_targets.worker, &resolved_targets.reviewer) {
            job.transition(CoreDelegationStatus::AwaitingApproval)
                .map_err(|error| error.to_string())?;
            job.set_error(
                "The approved provider or worker configuration changed. Review the target and approve a fresh attempt.",
            );
            store
                .update(job.clone())
                .map_err(|error| error.to_string())?;
            self.emit(&store, &job, EventKind::ApprovalRequired);
            return Ok(());
        }
        job.transition(CoreDelegationStatus::WorkerRunning)
            .map_err(|error| error.to_string())?;
        let worker_agent = resolved_targets.worker.target.identity().to_string();
        let worker_attempt = job
            .start_attempt_with_metadata(
                AttemptRole::Worker,
                &worker_agent,
                resolved_targets.worker.clone(),
            )
            .map_err(|error| error.to_string())?;
        store
            .update(job.clone())
            .map_err(|error| error.to_string())?;
        self.emit(&store, &job, EventKind::WorkerStarted);
        let worker_heartbeat = self.start_heartbeat_loop(
            root.to_path_buf(),
            job_id.to_string(),
            cancel.clone(),
            "worker dispatch is still active".into(),
        );

        let snapshot = job.base_workspace_snapshot.clone();
        let dependency_summary = dependencies
            .iter()
            .filter(|candidate| job.card.depends_on.iter().any(|id| id == &candidate.job_id))
            .map(|candidate| {
                format!(
                    "{}: {:?} — {}",
                    candidate.job_id, candidate.status, candidate.card.title
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let dependency_summary = dependency_summary.chars().take(12_000).collect::<String>();
        let worker_prompt = job.card.prompt(root, &snapshot, &dependency_summary);
        let worker_prompt = if job.attempt > 1 {
            let previous_diff = store
                .read_artifact(job_id, "worker.diff")
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .map(|diff| diff.chars().take(24_000).collect::<String>())
                .unwrap_or_else(|| "(previous worker diff unavailable)".into());
            let previous_review = store
                .read_artifact(job_id, "review-result.json")
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .map(|review| review.chars().take(16_000).collect::<String>())
                .unwrap_or_else(|| "(previous reviewer report unavailable)".into());
            format!(
                "{worker_prompt}\n\n# Fresh-fix context\nThis is a new worker attempt. Preserve useful work where appropriate, but independently address the findings below. The previous worker diff and reviewer report are evidence only.\n\n## Previous worker diff\n```diff\n{previous_diff}\n```\n\n## Previous reviewer report\n```json\n{previous_review}\n```"
            )
        } else {
            worker_prompt
        };
        let worker_target = resolved_targets.worker.target.clone();
        let mut worker_evidence = None;
        let worker = match &worker_target {
            DelegationTarget::ExternalAgent { agent_id } => {
                let agent = config
                    .agents
                    .get(agent_id)
                    .ok_or_else(|| format!("external worker `{agent_id}` is unavailable"))?;
                let result = run_delegation_worker(
                    root,
                    agent,
                    &worker_prompt,
                    Some(cancel),
                    &parent_secret_envs,
                )
                .await
                .map_err(|error| error.to_string())?;
                if let Ok(mut ledger) = self.ledger.lock() {
                    ledger.record_external(agent_id, result.usage.as_ref());
                }
                let attempt_usage = result.usage.as_ref().map(attempt_usage_from_external);
                let parsed = WorkerResult::from_external(&result.text, &result.diff)
                    .ok_or_else(|| "worker returned no usable result".to_string())?;
                if let Some(mut evidence) = result.session_evidence {
                    evidence.worker_id = agent_id.clone();
                    evidence.preview = Some(parsed.summary.clone());
                    worker_evidence = Some(evidence);
                }
                Ok((parsed, result.diff, result.text, attempt_usage))
            }
            DelegationTarget::Provider { .. } => {
                let result = run_provider_worker(
                    root,
                    config.clone(),
                    &worker_target,
                    &worker_prompt,
                    Some(self.ledger.clone()),
                    Some(cancel),
                )
                .await
                .map_err(|error| error.to_string())?;
                Ok((
                    result.result,
                    result.diff,
                    result.final_text,
                    Some(result.usage),
                ))
            }
        };
        worker_heartbeat.abort();
        if cancel.is_cancelled() {
            return self.cancelled(&store, job).await;
        }
        let (worker_result, worker_diff, worker_text, worker_usage) =
            worker.map_err(|error: String| format!("worker failed: {error}"))?;
        if worker_diff.trim().is_empty() {
            return Err("worker returned no diff artifact".into());
        }
        validate_diff_scope(root, &worker_diff, &job.card.scope)
            .map_err(|error| format!("worker produced an unsafe or out-of-scope diff: {error}"))?;
        store
            .write_artifact(job_id, "worker.diff", worker_diff.as_bytes())
            .map_err(|error| error.to_string())?;
        store
            .write_artifact(
                job_id,
                "worker-result.json",
                &serde_json::to_vec_pretty(&worker_result).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
        job.finish_attempt(&worker_attempt);
        if let Some(usage) = worker_usage {
            job.set_attempt_usage(&worker_attempt, usage)
                .map_err(|e| e.to_string())?;
        }
        if let Some(evidence) = worker_evidence.take() {
            job.orchestration.attach_external_session(evidence);
        }
        store
            .update(job.clone())
            .map_err(|error| error.to_string())?;
        let _ = worker_text;
        self.emit(&store, &job, EventKind::WorkerCompleted);
        if cancel.is_cancelled() {
            return self.cancelled(&store, job).await;
        }
        job.transition(CoreDelegationStatus::ReviewRunning)
            .map_err(|error| error.to_string())?;
        let reviewer_target = resolved_targets.reviewer.target.clone();
        let reviewer_agent = reviewer_target.identity().to_string();
        let reviewer_attempt = job
            .start_attempt_with_metadata(
                AttemptRole::Reviewer,
                &reviewer_agent,
                resolved_targets.reviewer.clone(),
            )
            .map_err(|error| error.to_string())?;
        store
            .update(job.clone())
            .map_err(|error| error.to_string())?;
        self.emit(&store, &job, EventKind::ReviewerStarted);
        let reviewer_heartbeat = self.start_heartbeat_loop(
            root.to_path_buf(),
            job_id.to_string(),
            cancel.clone(),
            "review dispatch is still active".into(),
        );
        let checks = run_acceptance_checks(
            root,
            config.clone(),
            &worker_diff,
            &job.card.acceptance_checks,
            Some(cancel),
        )
        .await
        .map_err(|error| format!("acceptance checks failed: {error}"))?;
        let check_evidence = serde_json::to_string_pretty(&checks).map_err(|e| e.to_string())?;
        let review_prompt = format!("{}\n\n# Authoritative acceptance-check results\n```json\n{}\n```\nReport these exact command/status/output values in your checks array.", job.card.review_prompt(root, &snapshot, &worker_result), check_evidence);
        let mut reviewer_evidence = None;
        let review = match &reviewer_target {
            DelegationTarget::ExternalAgent { agent_id } => {
                let agent = config
                    .agents
                    .get(agent_id)
                    .ok_or_else(|| format!("external reviewer `{agent_id}` is unavailable"))?;
                let result = run_delegation_reviewer(
                    root,
                    agent,
                    &worker_diff,
                    &review_prompt,
                    Some(cancel),
                    &parent_secret_envs,
                )
                .await
                .map_err(|error| format!("reviewer failed: {error}"))?;
                if let Ok(mut ledger) = self.ledger.lock() {
                    ledger.record_external(agent_id, result.usage.as_ref());
                }
                let attempt_usage = result.usage.as_ref().map(attempt_usage_from_external);
                if let Some(mut evidence) = result.session_evidence {
                    evidence.worker_id = agent_id.clone();
                    reviewer_evidence = Some(evidence);
                }
                (result.text, result.diff, attempt_usage)
            }
            DelegationTarget::Provider { .. } => {
                let result = run_provider_reviewer(
                    root,
                    config.clone(),
                    &reviewer_target,
                    &worker_diff,
                    &review_prompt,
                    Some(self.ledger.clone()),
                    Some(cancel),
                )
                .await
                .map_err(|error| format!("reviewer failed: {error}"))?;
                (result.final_text, result.reviewer_diff, Some(result.usage))
            }
        };
        reviewer_heartbeat.abort();
        if let Some(mut evidence) = reviewer_evidence.take() {
            evidence.preview = Some(review.0.chars().take(8_000).collect());
            job.orchestration.attach_external_session(evidence);
            store
                .update(job.clone())
                .map_err(|error| error.to_string())?;
        }
        if cancel.is_cancelled() {
            return self.cancelled(&store, job).await;
        }
        if !review.1.trim().is_empty() {
            let discarded = serde_json::json!({
                "error": "reviewer produced edits; the reviewer diff was discarded",
                "raw": review.0,
            });
            store
                .write_artifact(
                    job_id,
                    "review-result.json",
                    &serde_json::to_vec_pretty(&discarded).map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
            job.finish_attempt(&reviewer_attempt);
            if let Some(usage) = review.2 {
                job.set_attempt_usage(&reviewer_attempt, usage)
                    .map_err(|e| e.to_string())?;
            }
            job.transition(CoreDelegationStatus::Blocked)
                .map_err(|error| error.to_string())?;
            job.set_error(
                "Reviewer produced edits. They were discarded, and a fresh reviewer is required.",
            );
            store
                .update(job.clone())
                .map_err(|error| error.to_string())?;
            self.emit(&store, &job, EventKind::Blocked);
            return Ok(());
        }
        let report = match ReviewReport::parse(&review.0, &job.card.acceptance_checks) {
            Ok(report) => report,
            Err(error) => {
                let malformed = serde_json::json!({"error": error.to_string(), "raw": review.0});
                store
                    .write_artifact(
                        job_id,
                        "review-result.json",
                        &serde_json::to_vec_pretty(&malformed)
                            .map_err(|error| error.to_string())?,
                    )
                    .map_err(|error| error.to_string())?;
                job.finish_attempt(&reviewer_attempt);
                if let Some(usage) = review.2.clone() {
                    job.set_attempt_usage(&reviewer_attempt, usage)
                        .map_err(|e| e.to_string())?;
                }
                job.transition(CoreDelegationStatus::Blocked)
                    .map_err(|error| error.to_string())?;
                job.set_error(error.to_string());
                store
                    .update(job.clone())
                    .map_err(|error| error.to_string())?;
                self.emit(&store, &job, EventKind::Blocked);
                return Ok(());
            }
        };
        if let Err(error) = zest_core::validate_review_paths(root, &report) {
            store
                .write_artifact(
                    job_id,
                    "review-result.json",
                    &serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
            job.finish_attempt(&reviewer_attempt);
            job.transition(CoreDelegationStatus::Blocked)
                .map_err(|error| error.to_string())?;
            job.set_error(error.to_string());
            store
                .update(job.clone())
                .map_err(|error| error.to_string())?;
            self.emit(&store, &job, EventKind::Blocked);
            return Ok(());
        }
        if let Err(error) = validate_authoritative_checks(&checks, &report) {
            store
                .write_artifact(
                    job_id,
                    "review-result.json",
                    &serde_json::to_vec_pretty(&serde_json::json!({
                        "error": error,
                        "report": report,
                        "authoritativeChecks": checks,
                    }))
                    .map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
            job.finish_attempt(&reviewer_attempt);
            if let Some(usage) = review.2 {
                job.set_attempt_usage(&reviewer_attempt, usage)
                    .map_err(|e| e.to_string())?;
            }
            job.transition(CoreDelegationStatus::Blocked)
                .map_err(|error| error.to_string())?;
            job.set_error(error);
            store
                .update(job.clone())
                .map_err(|error| error.to_string())?;
            self.emit(&store, &job, EventKind::Blocked);
            return Ok(());
        }
        store
            .write_artifact(
                job_id,
                "review-result.json",
                &serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
        job.finish_attempt(&reviewer_attempt);
        if let Some(usage) = review.2 {
            job.set_attempt_usage(&reviewer_attempt, usage)
                .map_err(|e| e.to_string())?;
        }
        store
            .update(job.clone())
            .map_err(|error| error.to_string())?;
        self.emit(&store, &job, EventKind::ReviewerCompleted);
        if report.can_accept(&job.card.acceptance_checks) {
            job.transition(CoreDelegationStatus::ReadyToApply)
                .map_err(|error| error.to_string())?;
            job.error = None;
            store
                .update(job.clone())
                .map_err(|error| error.to_string())?;
            self.emit(&store, &job, EventKind::ReadyToApply);
        } else {
            job.transition(CoreDelegationStatus::ChangesRequested)
                .map_err(|error| error.to_string())?;
            job.set_error("Reviewer requested changes before this diff can be applied.");
            store
                .update(job.clone())
                .map_err(|error| error.to_string())?;
            self.emit(&store, &job, EventKind::ChangesRequested);
        }
        Ok(())
    }

    async fn cancelled(
        &self,
        store: &DelegationStore,
        mut job: DelegationJob,
    ) -> Result<(), String> {
        if self.shutting_down.load(Ordering::SeqCst) {
            if !job.status.is_terminal() {
                job.finish_active_attempts();
                job.transition(CoreDelegationStatus::Blocked)
                    .map_err(|error| error.to_string())?;
                job.set_error(
                    "the coordinator stopped while this job was running; review and retry",
                );
                store
                    .update(job.clone())
                    .map_err(|error| error.to_string())?;
                self.emit(store, &job, EventKind::Blocked);
            }
            return Ok(());
        }
        if !job.status.is_terminal() {
            job.finish_active_attempts();
            job.transition(CoreDelegationStatus::Cancelled)
                .map_err(|error| error.to_string())?;
            store
                .update(job.clone())
                .map_err(|error| error.to_string())?;
            self.emit(store, &job, EventKind::Cancelled);
        }
        Ok(())
    }

    fn fail(
        self: &Arc<Self>,
        root: &Path,
        job_id: &str,
        error: &str,
        kind: EventKind,
    ) -> ResultView {
        let store = DelegationStore::open(root).map_err(|error| error.to_string())?;
        let mut job = store
            .load(job_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "delegation job was not found".to_string())?;
        if !job.status.is_terminal() {
            let next = if matches!(kind, EventKind::Blocked) {
                CoreDelegationStatus::Blocked
            } else {
                CoreDelegationStatus::Failed
            };
            job.transition(next).map_err(|error| error.to_string())?;
            job.finish_active_attempts_with_status(
                zest_core::DispatchStatus::Failed,
                "dispatch failed",
            );
            job.set_error(error);
            store
                .update(job.clone())
                .map_err(|error| error.to_string())?;
            self.emit(&store, &job, kind);
            self.kick_pending(root, &store);
        }
        Ok(job_view(&store, &job))
    }

    fn emit(&self, store: &DelegationStore, job: &DelegationJob, kind: EventKind) {
        if matches!(
            job.status,
            CoreDelegationStatus::ReadyToApply
                | CoreDelegationStatus::Accepted
                | CoreDelegationStatus::ChangesRequested
                | CoreDelegationStatus::Blocked
                | CoreDelegationStatus::Failed
                | CoreDelegationStatus::Cancelled
                | CoreDelegationStatus::ApplyConflict
        ) {
            if let Ok(mut running) = self.running.lock() {
                running.remove(&job.job_id);
            }
        }
        let view = job_view(store, job);
        let event = match kind {
            EventKind::CardCreated => DelegationEvent::CardCreated { job: view },
            EventKind::ApprovalRequired => DelegationEvent::ApprovalRequired { job: view },
            EventKind::Queued => DelegationEvent::Queued { job: view },
            EventKind::WorkerStarted => DelegationEvent::WorkerStarted { job: view },
            EventKind::Heartbeat => DelegationEvent::Heartbeat { job: view },
            EventKind::WorkerCompleted => DelegationEvent::WorkerCompleted { job: view },
            EventKind::ReviewerStarted => DelegationEvent::ReviewerStarted { job: view },
            EventKind::ReviewerCompleted => DelegationEvent::ReviewerCompleted { job: view },
            EventKind::ChangesRequested => DelegationEvent::ChangesRequested { job: view },
            EventKind::ReadyToApply => DelegationEvent::ReadyToApply { job: view },
            EventKind::Applied => DelegationEvent::Applied { job: view },
            EventKind::Conflict => DelegationEvent::Conflict { job: view },
            EventKind::Blocked => DelegationEvent::Blocked { job: view },
            EventKind::Failed => DelegationEvent::Failed { job: view },
            EventKind::Cancelled => DelegationEvent::Cancelled { job: view },
        };
        self.notifier.notify(event);
    }
}

impl Default for DelegationCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

pub type ResultView = Result<DelegationJobView, String>;

pub fn list_views(root: &Path) -> Result<Vec<DelegationJobView>, String> {
    let store = DelegationStore::open(root).map_err(|error| error.to_string())?;
    let jobs = store.list().map_err(|error| error.to_string())?;
    Ok(jobs.iter().map(|job| job_view(&store, job)).collect())
}

pub fn get_view(root: &Path, job_id: &str) -> Result<DelegationJobView, String> {
    let store = DelegationStore::open(root).map_err(|error| error.to_string())?;
    let job = store
        .load(job_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "delegation job was not found".to_string())?;
    Ok(job_view(&store, &job))
}

fn canonicalize_root(root: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(root).map_err(|error| format!("could not resolve project root: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::Arc;

    use zest_core::{FeatureCard, DELEGATION_FORMAT_VERSION};

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn fixture_binary() -> &'static PathBuf {
        static FIXTURE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        FIXTURE.get_or_init(|| {
            let source = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("core")
                .join("tests")
                .join("fixtures")
                .join("external_agent_fixture.rs");
            let output = std::env::temp_dir().join(format!(
                "zest-coordinator-delegation-fixture-{}{}",
                std::process::id(),
                std::env::consts::EXE_SUFFIX
            ));
            let result = Command::new("rustc")
                .args(["--edition=2021"])
                .arg(source)
                .arg("-o")
                .arg(&output)
                .output()
                .expect("start rustc for delegation fixture");
            assert!(
                result.status.success(),
                "delegation fixture compilation failed: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            output
        })
    }

    fn write_fixture_project(root: &Path) {
        git(root, &["init", "--quiet"]);
        git(root, &["config", "user.name", "Zest Test"]);
        git(root, &["config", "user.email", "zest-test@localhost"]);
        std::fs::write(root.join("README.md"), "fixture project\n").unwrap();
        git(root, &["add", "."]);
        git(
            root,
            &["commit", "--quiet", "--no-verify", "-m", "baseline"],
        );
        let command =
            serde_json::to_string(&fixture_binary().to_string_lossy().to_string()).unwrap();
        std::fs::write(
            root.join("zest.toml"),
            format!(
                "[agents.worker]\nmode = \"headless\"\ncommand = {command}\nargs = [\"delegation\", \"{{prompt}}\"]\nworkspace = \"isolated\"\ntimeout_secs = 30\n"
            ),
        )
        .unwrap();
    }

    fn create_request(title: &str) -> CreateDelegationJobRequest {
        CreateDelegationJobRequest {
            parent_thread_id: "thread-fixture".into(),
            title: title.into(),
            objective: "Create the fixture change".into(),
            lane: "test".into(),
            scope: vec![".".into()],
            context: vec![],
            depends_on: vec![],
            acceptance_checks: vec![],
            worker: DelegationTargetView::ExternalAgent {
                agent_id: "worker".into(),
            },
            reviewer: None,
            chat_id: None,
            idempotency_key: None,
            origin_coordinator: None,
        }
    }

    async fn wait_for_status(
        coordinator: &Arc<DelegationCoordinator>,
        root: &Path,
        job_id: &str,
        wanted: DelegationStatus,
    ) -> DelegationJobView {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let view = get_view(root, job_id).unwrap();
            if view.status == wanted {
                return view;
            }
            if matches!(
                view.status,
                DelegationStatus::Failed
                    | DelegationStatus::Blocked
                    | DelegationStatus::Cancelled
                    | DelegationStatus::ApplyConflict
            ) && wanted != view.status
                && view.status != DelegationStatus::AwaitingApproval
            {
                panic!(
                    "job {} reached {:?} error={:?}",
                    job_id, view.status, view.error
                );
            }
            if tokio::time::Instant::now() > deadline {
                panic!(
                    "timed out waiting for {:?}; last status={:?} error={:?}",
                    wanted, view.status, view.error
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = coordinator.reconcile(root);
        }
    }

    #[tokio::test]
    async fn coordinator_runs_worker_review_and_apply() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write_fixture_project(root);
        let coordinator = Arc::new(DelegationCoordinator::new());
        let created = coordinator
            .create_job(root, create_request("Fixture delegation"))
            .unwrap();
        assert_eq!(created.status, DelegationStatus::AwaitingApproval);
        let approved = coordinator
            .approve(root, &created.job_id, Some(created.updated_at))
            .unwrap();
        assert_eq!(approved.status, DelegationStatus::Queued);
        let ready = wait_for_status(
            &coordinator,
            root,
            &created.job_id,
            DelegationStatus::ReadyToApply,
        )
        .await;
        let ready = get_view(root, &ready.job_id).unwrap();
        let applied = coordinator
            .apply(root, &ready.job_id, Some(ready.updated_at))
            .unwrap();
        assert_eq!(applied.status, DelegationStatus::Accepted);
        let again = coordinator
            .apply(root, &ready.job_id, Some(ready.updated_at))
            .unwrap();
        assert_eq!(again.status, DelegationStatus::Accepted);
        assert_eq!(
            std::fs::read_to_string(root.join("delegated.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "fixture worker change\n"
        );
    }

    #[tokio::test]
    async fn mcp_create_stays_awaiting_approval_without_a_receipt() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write_fixture_project(root);
        let coordinator = Arc::new(DelegationCoordinator::new());
        let mut request = create_request("MCP card");
        request.idempotency_key = Some("mcp-1".into());
        request.origin_coordinator = Some(INBOUND_MCP_ORIGIN.into());
        let created = coordinator.create_job(root, request.clone()).unwrap();
        assert_eq!(created.status, DelegationStatus::AwaitingApproval);
        assert!(!created.approved);
        assert_eq!(
            created
                .origin
                .as_ref()
                .map(|origin| origin.coordinator.as_str()),
            Some(INBOUND_MCP_ORIGIN)
        );
        let labeled = {
            let mut other = create_request("Named bot");
            other.idempotency_key = Some("mcp-bot-1".into());
            other.origin_coordinator = Some("my-bot".into());
            coordinator.create_job(root, other).unwrap()
        };
        assert_eq!(
            labeled
                .origin
                .as_ref()
                .map(|origin| origin.coordinator.as_str()),
            Some("my-bot")
        );
        assert_eq!(labeled.status, DelegationStatus::AwaitingApproval);
        let duplicate = coordinator.create_job(root, request).unwrap();
        assert_eq!(duplicate.job_id, created.job_id);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let still = get_view(root, &created.job_id).unwrap();
        assert_eq!(still.status, DelegationStatus::AwaitingApproval);
    }

    #[tokio::test]
    async fn interactive_receipt_is_ingested_on_reconcile() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write_fixture_project(root);
        let store = DelegationStore::open(root).unwrap();
        let card = FeatureCard {
            version: DELEGATION_FORMAT_VERSION,
            card_id: String::new(),
            title: "TUI card".into(),
            objective: "Create the fixture change".into(),
            lane: "test".into(),
            scope: vec![".".into()],
            context: vec![],
            depends_on: vec![],
            agent: "worker".into(),
            worker_target: Some(zest_core::DelegationTarget::ExternalAgent {
                agent_id: "worker".into(),
            }),
            acceptance_checks: vec![],
            review_required: true,
            reviewer_target: ReviewerTarget::SameAsWorker,
            created_at: 0,
        };
        let mut job = store
            .create(
                "thread-tui",
                card,
                zest_core::capture_workspace_snapshot(root),
            )
            .unwrap();
        job.grant_dispatch_receipt("delegate_feature");
        store.update(job.clone()).unwrap();
        let coordinator = Arc::new(DelegationCoordinator::new());
        coordinator.reconcile(root).unwrap();
        let ready = wait_for_status(
            &coordinator,
            root,
            &job.job_id,
            DelegationStatus::ReadyToApply,
        )
        .await;
        assert!(ready.approved);
    }

    #[tokio::test]
    async fn stale_revision_is_rejected_and_apply_conflict_keeps_the_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write_fixture_project(root);
        let coordinator = Arc::new(DelegationCoordinator::new());
        let created = coordinator
            .create_job(root, create_request("Conflict card"))
            .unwrap();
        assert!(coordinator
            .approve(
                root,
                &created.job_id,
                Some(created.updated_at.saturating_sub(1))
            )
            .is_err());
        let approved = coordinator.approve(root, &created.job_id, None).unwrap();
        let ready = wait_for_status(
            &coordinator,
            root,
            &approved.job_id,
            DelegationStatus::ReadyToApply,
        )
        .await;
        std::fs::write(root.join("delegated.txt"), "conflicting\n").unwrap();
        git(root, &["add", "delegated.txt"]);
        git(
            root,
            &["commit", "--quiet", "--no-verify", "-m", "conflict"],
        );
        let conflicted = coordinator
            .apply(root, &ready.job_id, Some(ready.updated_at))
            .unwrap();
        assert_eq!(conflicted.status, DelegationStatus::ApplyConflict);
        assert_eq!(
            std::fs::read_to_string(root.join("delegated.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "conflicting\n"
        );
    }

    #[tokio::test]
    async fn exclusive_lock_rejects_a_second_coordinator() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write_fixture_project(root);
        let first = Arc::new(DelegationCoordinator::new());
        first.ensure_lock(root).unwrap();
        let second = Arc::new(DelegationCoordinator::new());
        let error = second.ensure_lock(root).unwrap_err();
        assert!(error.contains("already owns this project"), "{error}");
    }

    #[tokio::test]
    async fn restart_blocks_an_interrupted_worker_and_resumes_queued_jobs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write_fixture_project(root);
        let store = DelegationStore::open(root).unwrap();
        let coordinator = Arc::new(DelegationCoordinator::new());
        let interrupted = coordinator
            .create_job(root, create_request("Interrupted"))
            .unwrap();
        drop(coordinator);

        let mut running = store.load(&interrupted.job_id).unwrap().unwrap();
        running.approve().unwrap();
        running.transition(CoreDelegationStatus::Queued).unwrap();
        running
            .transition(CoreDelegationStatus::WorkerRunning)
            .unwrap();
        store.update(running.clone()).unwrap();

        let restarted = Arc::new(DelegationCoordinator::new());
        restarted.reconcile(root).unwrap();
        let blocked = get_view(root, &interrupted.job_id).unwrap();
        assert_eq!(blocked.status, DelegationStatus::Blocked);

        let queued = restarted
            .create_job(root, create_request("Queued recovery"))
            .unwrap();
        drop(restarted);
        let mut pending = store.load(&queued.job_id).unwrap().unwrap();
        pending.approve().unwrap();
        pending.transition(CoreDelegationStatus::Queued).unwrap();
        store.update(pending.clone()).unwrap();

        let resumed = Arc::new(DelegationCoordinator::new());
        resumed.reconcile(root).unwrap();
        let mut current = wait_until_not_queued(&resumed, root, &queued.job_id).await;
        if current.status == DelegationStatus::AwaitingApproval {
            current = resumed.approve(root, &queued.job_id, None).unwrap();
        }
        if current.status != DelegationStatus::ReadyToApply
            && current.status != DelegationStatus::Accepted
        {
            wait_for_status(
                &resumed,
                root,
                &queued.job_id,
                DelegationStatus::ReadyToApply,
            )
            .await;
        }
    }

    async fn wait_until_not_queued(
        coordinator: &Arc<DelegationCoordinator>,
        root: &Path,
        job_id: &str,
    ) -> DelegationJobView {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let view = get_view(root, job_id).unwrap();
            if view.status != DelegationStatus::Queued {
                return view;
            }
            if tokio::time::Instant::now() > deadline {
                panic!("job {job_id} stayed queued");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = coordinator.reconcile(root);
        }
    }

    #[tokio::test]
    async fn cancel_is_idempotent_and_dependencies_block_until_accepted() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write_fixture_project(root);
        let coordinator = Arc::new(DelegationCoordinator::new());
        let first = coordinator
            .create_job(root, create_request("First"))
            .unwrap();
        let mut second_req = create_request("Second");
        second_req.depends_on = vec![first.job_id.clone()];
        let second = coordinator.create_job(root, second_req).unwrap();
        coordinator.approve(root, &second.job_id, None).unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        let waiting = get_view(root, &second.job_id).unwrap();
        assert!(
            matches!(
                waiting.status,
                DelegationStatus::Queued | DelegationStatus::Blocked
            ),
            "{:?}",
            waiting.status
        );
        let cancelled = coordinator
            .cancel(root, &second.job_id, Some(waiting.updated_at))
            .unwrap();
        assert_eq!(cancelled.status, DelegationStatus::Cancelled);
        let again = coordinator
            .cancel(root, &second.job_id, Some(waiting.updated_at))
            .unwrap();
        assert_eq!(again.status, DelegationStatus::Cancelled);
    }

    #[tokio::test]
    async fn concurrent_approve_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        write_fixture_project(&root);
        let coordinator = Arc::new(DelegationCoordinator::new());
        let created = coordinator
            .create_job(&root, create_request("Concurrent approve"))
            .unwrap();
        let job_id = created.job_id.clone();
        let expected = created.updated_at;
        let first = coordinator.clone();
        let second = coordinator.clone();
        let root_a = root.clone();
        let root_b = root.clone();
        let id_a = job_id.clone();
        let id_b = job_id.clone();
        let (left, right) = tokio::join!(
            tokio::task::spawn_blocking(move || first.approve(&root_a, &id_a, Some(expected))),
            tokio::task::spawn_blocking(move || second.approve(&root_b, &id_b, Some(expected))),
        );
        let left = left.expect("first approve task");
        let right = right.expect("second approve task");
        assert!(left.is_ok(), "{left:?}");
        assert!(right.is_ok(), "{right:?}");
        let left = left.unwrap();
        let right = right.unwrap();
        assert_eq!(left.job_id, right.job_id);
        assert_ne!(left.status, DelegationStatus::AwaitingApproval);
        assert_ne!(right.status, DelegationStatus::AwaitingApproval);
        wait_for_status(&coordinator, &root, &job_id, DelegationStatus::ReadyToApply).await;
    }
}
