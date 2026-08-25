//! Runtime half of the durable thread input queue.
//!
//! [`Thread`](crate::Thread) owns the persisted projection. `InputInbox` is the
//! small live bridge used by an active agent so a steer or inject can be
//! accepted without making the model loop poll the filesystem. The desktop
//! installs a claim observer to remove the same item from the thread snapshot
//! before it is delivered to the provider.

use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::{ThreadInput, ThreadInputTarget};

pub type ClaimObserver = Arc<dyn Fn(Vec<ThreadInput>) + Send + Sync>;

#[derive(Clone, Default)]
pub struct InputInbox {
    inner: Arc<Mutex<Vec<ThreadInput>>>,
    notify: Arc<Notify>,
    observer: Arc<Mutex<Option<ClaimObserver>>>,
}

impl std::fmt::Debug for InputInbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputInbox")
            .field("pending", &self.snapshot().len())
            .finish()
    }
}

impl InputInbox {
    pub fn from_pending(inputs: impl IntoIterator<Item = ThreadInput>) -> Self {
        let inbox = Self::default();
        if let Ok(mut pending) = inbox.inner.lock() {
            pending.extend(inputs);
        }
        inbox
    }

    /// Install the durable projection hook. The hook runs after the live list
    /// has released its mutex, so it may lock the thread and enqueue a
    /// persistence write without a lock inversion.
    pub fn set_claim_observer(&self, observer: ClaimObserver) {
        if let Ok(mut current) = self.observer.lock() {
            *current = Some(observer);
        }
    }

    pub fn enqueue(&self, input: ThreadInput) {
        if let Ok(mut pending) = self.inner.lock() {
            pending.push(input);
        }
        self.notify.notify_waiters();
    }

    pub fn snapshot(&self) -> Vec<ThreadInput> {
        self.inner
            .lock()
            .map(|pending| pending.clone())
            .unwrap_or_default()
    }

    /// Keep the live projection aligned with an edit made to the durable
    /// thread while a turn is running.
    pub fn update_text(&self, input_id: &str, text: impl Into<String>) -> bool {
        let text = text.into();
        let updated = self
            .inner
            .lock()
            .map(|mut pending| {
                let Some(input) = pending.iter_mut().find(|input| input.id == input_id) else {
                    return false;
                };
                input.text = text;
                true
            })
            .unwrap_or(false);
        if updated {
            self.notify.notify_waiters();
        }
        updated
    }

    /// Remove an input from the live projection without firing the claim
    /// observer. This is a user cancellation, not a delivery claim.
    pub fn remove(&self, input_id: &str) -> bool {
        let removed = self
            .inner
            .lock()
            .map(|mut pending| {
                let Some(index) = pending.iter().position(|input| input.id == input_id) else {
                    return false;
                };
                pending.remove(index);
                true
            })
            .unwrap_or(false);
        if removed {
            self.notify.notify_waiters();
        }
        removed
    }

    pub fn is_empty(&self) -> bool {
        self.inner
            .lock()
            .map(|pending| pending.is_empty())
            .unwrap_or(true)
    }

    /// Claim all step-scoped entries, preserving FIFO order among them.
    pub fn claim_next_step(&self) -> Vec<ThreadInput> {
        let claimed = match self.inner.lock() {
            Ok(mut pending) => {
                let mut claimed = Vec::new();
                let mut remaining = Vec::with_capacity(pending.len());
                for input in pending.drain(..) {
                    if matches!(
                        input.target,
                        ThreadInputTarget::Steer | ThreadInputTarget::Inject
                    ) {
                        claimed.push(input);
                    } else {
                        remaining.push(input);
                    }
                }
                *pending = remaining;
                claimed
            }
            Err(_) => Vec::new(),
        };
        self.observe_claim(&claimed);
        claimed
    }

    /// Claim one followup at the turn boundary. Other followups remain FIFO.
    pub fn claim_followup(&self) -> Option<ThreadInput> {
        let claimed = match self.inner.lock() {
            Ok(mut pending) => {
                let index = pending
                    .iter()
                    .position(|input| input.target == ThreadInputTarget::Followup)?;
                Some(pending.remove(index))
            }
            Err(_) => None,
        };
        if let Some(input) = claimed.clone() {
            self.observe_claim(&[input]);
        }
        claimed
    }

    pub fn notify(&self) {
        self.notify.notify_waiters();
    }

    /// Wait until a live message arrives. This is intentionally separate from
    /// claiming: an agent driver can use it to wake, while the next step still
    /// decides which target is eligible.
    pub async fn wait_for_message(&self) {
        self.notify.notified().await;
    }

    fn observe_claim(&self, claimed: &[ThreadInput]) {
        if claimed.is_empty() {
            return;
        }
        let observer = self
            .observer
            .lock()
            .ok()
            .and_then(|current| current.clone());
        if let Some(observer) = observer {
            observer(claimed.to_vec());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(id: &str, target: ThreadInputTarget) -> ThreadInput {
        ThreadInput {
            id: id.into(),
            target,
            text: id.into(),
            created_at: 1,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn claims_step_inputs_without_consuming_followups() {
        let inbox = InputInbox::from_pending([
            input("followup-1", ThreadInputTarget::Followup),
            input("steer-1", ThreadInputTarget::Steer),
            input("inject-1", ThreadInputTarget::Inject),
            input("followup-2", ThreadInputTarget::Followup),
        ]);

        let claimed = inbox.claim_next_step();
        assert_eq!(
            claimed
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["steer-1", "inject-1"]
        );
        assert_eq!(inbox.claim_followup().unwrap().id, "followup-1");
        assert_eq!(inbox.claim_followup().unwrap().id, "followup-2");
        assert!(inbox.claim_followup().is_none());
    }

    #[test]
    fn claim_observer_runs_after_removal() {
        let inbox = InputInbox::from_pending([input("steer", ThreadInputTarget::Steer)]);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let copy = seen.clone();
        inbox.set_claim_observer(Arc::new(move |claimed| {
            copy.lock()
                .unwrap()
                .push((claimed[0].id.clone(), claimed.len()));
        }));
        let claimed = inbox.claim_next_step();
        assert!(inbox.is_empty());
        assert_eq!(seen.lock().unwrap().as_slice(), [("steer".into(), 1)]);
        assert_eq!(claimed.len(), 1);
    }
}
