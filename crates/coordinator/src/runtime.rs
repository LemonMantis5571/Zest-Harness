use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::scheduler::DelegationEvent;

pub type BoxFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

pub trait SpawnAbort: Send + Sync {
    fn abort(&self);
}

pub trait TaskSpawner: Send + Sync {
    fn spawn(&self, fut: BoxFuture) -> Box<dyn SpawnAbort>;
}

pub trait DelegationNotifier: Send + Sync {
    fn notify(&self, event: DelegationEvent);
}

pub struct NoopNotifier;

impl DelegationNotifier for NoopNotifier {
    fn notify(&self, _event: DelegationEvent) {}
}

pub struct RecordingNotifier {
    events: Mutex<Vec<DelegationEvent>>,
}

impl RecordingNotifier {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            events: Mutex::new(Vec::new()),
        })
    }

    pub fn events(&self) -> Vec<DelegationEvent> {
        self.events
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default()
    }
}

impl DelegationNotifier for RecordingNotifier {
    fn notify(&self, event: DelegationEvent) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
    }
}

struct TokioAbort(tokio::task::AbortHandle);

impl SpawnAbort for TokioAbort {
    fn abort(&self) {
        self.0.abort();
    }
}

/// Spawns onto a captured Tokio runtime handle.
pub struct TokioSpawner {
    handle: tokio::runtime::Handle,
}

impl TokioSpawner {
    pub fn current() -> Self {
        Self {
            handle: tokio::runtime::Handle::current(),
        }
    }

    pub fn from_handle(handle: tokio::runtime::Handle) -> Self {
        Self { handle }
    }
}

impl TaskSpawner for TokioSpawner {
    fn spawn(&self, fut: BoxFuture) -> Box<dyn SpawnAbort> {
        let join = self.handle.spawn(fut);
        Box::new(TokioAbort(join.abort_handle()))
    }
}

pub struct SharedNotifier {
    inner: Mutex<Arc<dyn DelegationNotifier>>,
}

impl SharedNotifier {
    pub fn new(notifier: Arc<dyn DelegationNotifier>) -> Self {
        Self {
            inner: Mutex::new(notifier),
        }
    }

    pub fn set(&self, notifier: Arc<dyn DelegationNotifier>) {
        if let Ok(mut inner) = self.inner.lock() {
            *inner = notifier;
        }
    }

    pub fn notify(&self, event: DelegationEvent) {
        if let Ok(inner) = self.inner.lock() {
            inner.notify(event);
        }
    }
}
