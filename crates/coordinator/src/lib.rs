//! Shared coordinator for durable feature-card jobs.
//!
//! Desktop and `zest serve` use the same scheduler. The durable store and
//! state machine stay in `zest-core`.

mod lock;
mod runtime;
mod scheduler;

pub use lock::{is_lock_held_error, lock_path, CoordinatorLock, LOCK_HELD_MESSAGE};
pub use runtime::{
    BoxFuture, DelegationNotifier, NoopNotifier, RecordingNotifier, SharedNotifier, SpawnAbort,
    TaskSpawner, TokioSpawner,
};
pub use scheduler::{
    get_view, job_view, list_views, AcceptanceCheckStatus, AcceptanceCheckView, ArtifactPage,
    AttemptUsageView, CreateDelegationJobRequest, DelegationAttemptView, DelegationCoordinator,
    DelegationEvent, DelegationHandoff, DelegationJobView, DelegationOriginView, DelegationStatus,
    DelegationTargetOption, DelegationTargetView, LockRetention, ResultView, ReviewFinding,
    ReviewSeverity, ReviewerTargetView, UpdateDelegationJobRequest, ALLOWED_ARTIFACTS,
    ARTIFACT_PAGE_BYTES, DESKTOP_ORIGIN, INBOUND_MCP_ORIGIN,
};
