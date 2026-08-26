//! Cooperative async cancellation for agent turns.
//!
//! Desktop arms a token when the user hits Stop. The agent races the token
//! against streaming, tool execution, and approval waits. Dropping an in-flight
//! HTTP body future aborts the connection.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;

/// Shared cancel signal for one in-flight turn.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    inner: Arc<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark cancelled and wake every waiter.
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// Completes when [`Self::cancel`] has been called.
    pub async fn cancelled(&self) {
        // Fast path.
        if self.is_cancelled() {
            return;
        }
        loop {
            let notified = self.inner.notify.notified();
            // Re-check after registering to avoid missing a wake.
            if self.is_cancelled() {
                return;
            }
            notified.await;
            if self.is_cancelled() {
                return;
            }
        }
    }
}

/// Await cancel or never, for use inside `tokio::select!`.
pub async fn wait_cancel(cancel: Option<&CancelToken>) {
    match cancel {
        Some(token) => token.cancelled().await,
        None => std::future::pending::<()>().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancel_wakes_waiter() {
        let token = CancelToken::new();
        let waiter = token.clone();
        let handle = tokio::spawn(async move {
            waiter.cancelled().await;
        });
        tokio::task::yield_now().await;
        token.cancel();
        handle.await.unwrap();
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn already_cancelled_is_ready() {
        let token = CancelToken::new();
        token.cancel();
        token.cancelled().await;
    }
}
