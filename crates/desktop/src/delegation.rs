//! Desktop adapter over the shared delegation coordinator.

use std::sync::Arc;

use tauri::{AppHandle, Emitter, Runtime};
use zest_coordinator::{BoxFuture, DelegationNotifier, SpawnAbort, TaskSpawner};

#[allow(unused_imports)]
pub use zest_coordinator::{
    get_view, list_views, AcceptanceCheckStatus, AcceptanceCheckView, AttemptUsageView,
    CreateDelegationJobRequest, DelegationAttemptView, DelegationCoordinator, DelegationEvent,
    DelegationHandoff, DelegationJobView, DelegationOriginView, DelegationStatus,
    DelegationTargetOption, DelegationTargetView, ReviewFinding, ReviewSeverity,
    ReviewerTargetView, UpdateDelegationJobRequest,
};

pub fn bind_tauri<R: Runtime>(coordinator: &DelegationCoordinator, app: AppHandle<R>) {
    coordinator.set_notifier(Arc::new(TauriDelegationNotifier { app }));
}

pub struct TauriSpawner;

impl TaskSpawner for TauriSpawner {
    fn spawn(&self, fut: BoxFuture) -> Box<dyn SpawnAbort> {
        let handle = tauri::async_runtime::spawn(fut);
        Box::new(TauriAbort(handle))
    }
}

struct TauriAbort(tauri::async_runtime::JoinHandle<()>);

impl SpawnAbort for TauriAbort {
    fn abort(&self) {
        self.0.abort();
    }
}

struct TauriDelegationNotifier<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> DelegationNotifier for TauriDelegationNotifier<R> {
    fn notify(&self, event: DelegationEvent) {
        let _ = self.app.emit("delegation-event", event);
    }
}
