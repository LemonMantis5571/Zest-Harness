//! Coalescing persistence for thread projections.
//!
//! User / tool / approval / terminal events flush immediately. Text and thinking
//! deltas are checkpointed at most every [`DELTA_CHECKPOINT_MS`] milliseconds.
//! Callers await the terminal flush so a completed turn is on disk before the
//! session controller clears busy state.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};

use crate::error::{HarnessError, Result};
use crate::thread::{Thread, ThreadStore};

/// Maximum delay before a text/thinking delta checkpoint is written.
pub const DELTA_CHECKPOINT_MS: u64 = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistPriority {
    /// User message, tool start/result, approval, done/error/cancelled.
    Immediate,
    /// Streaming text / thinking deltas — coalesced.
    Delta,
}

/// What to write, and when to read it.
pub enum Snapshot {
    /// A copy the caller already took. Use when the state at *this* instant is
    /// what has to land — a finished turn, a tool result.
    Owned(Box<Thread>),
    /// A handle to the live thread, read at write time.
    ///
    /// This is what makes coalescing worth anything. A burst of deltas produces
    /// one write, so copying the whole conversation on every delta duplicates a
    /// long transcript dozens of times a second and discards all but one of
    /// them. Handing over the handle defers the copy to the write that actually
    /// happens — and that copy is taken on the blocking thread, so the lock is
    /// never held on the async runtime.
    Live(Arc<std::sync::Mutex<Thread>>),
}

impl Snapshot {
    pub fn owned(thread: Thread) -> Self {
        Snapshot::Owned(Box::new(thread))
    }
}

enum Cmd {
    Upsert {
        snapshot: Snapshot,
        priority: PersistPriority,
        ack: Option<oneshot::Sender<Result<()>>>,
    },
    Forget {
        thread_id: String,
        ack: oneshot::Sender<Result<()>>,
    },
    Flush {
        ack: oneshot::Sender<Result<()>>,
    },
    Shutdown {
        ack: oneshot::Sender<()>,
    },
}

/// Background worker that owns a [`ThreadStore`] and coalesces delta writes.
#[derive(Clone)]
pub struct PersistWorker {
    tx: mpsc::UnboundedSender<Cmd>,
}

impl PersistWorker {
    pub fn spawn(workspace_root: impl Into<PathBuf>) -> Result<Self> {
        let root = workspace_root.into();
        let store = ThreadStore::open(&root)?;
        let (tx, rx) = mpsc::unbounded_channel();
        // Prefer the ambient Tokio runtime (desktop async commands). Fall back to
        // a dedicated thread so sync entrypoints can still construct a worker.
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(run_worker(store, rx));
            }
            Err(_) => {
                std::thread::Builder::new()
                    .name("zest-persist".into())
                    .spawn(move || {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("persist worker runtime");
                        rt.block_on(run_worker(store, rx));
                    })
                    .map_err(|e| HarnessError::Other(format!("spawn persist worker: {e}")))?;
            }
        }
        Ok(Self { tx })
    }

    /// Enqueue a thread snapshot. When `ack` is needed, use [`Self::save`] /
    /// [`Self::save_and_wait`] instead.
    pub fn enqueue(&self, snapshot: Snapshot, priority: PersistPriority) -> Result<()> {
        self.tx
            .send(Cmd::Upsert {
                snapshot,
                priority,
                ack: None,
            })
            .map_err(|_| HarnessError::Other("persist worker stopped".into()))
    }

    /// Save immediately (or as a delta) and wait for the write to finish.
    pub async fn save_and_wait(&self, snapshot: Snapshot, priority: PersistPriority) -> Result<()> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.tx
            .send(Cmd::Upsert {
                snapshot,
                priority,
                ack: Some(ack_tx),
            })
            .map_err(|_| HarnessError::Other("persist worker stopped".into()))?;
        ack_rx
            .await
            .map_err(|_| HarnessError::Other("persist worker dropped ack".into()))?
    }

    /// Stop writing a deleted chat. Later upserts for this id are ignored so a
    /// finishing turn cannot recreate the file after the sidebar reports
    /// success.
    pub async fn forget(&self, thread_id: impl Into<String>) -> Result<()> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.tx
            .send(Cmd::Forget {
                thread_id: thread_id.into(),
                ack: ack_tx,
            })
            .map_err(|_| HarnessError::Other("persist worker stopped".into()))?;
        ack_rx
            .await
            .map_err(|_| HarnessError::Other("persist worker dropped forget ack".into()))?
    }

    /// Force any pending coalesced snapshot to disk.
    pub async fn flush(&self) -> Result<()> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.tx
            .send(Cmd::Flush { ack: ack_tx })
            .map_err(|_| HarnessError::Other("persist worker stopped".into()))?;
        ack_rx
            .await
            .map_err(|_| HarnessError::Other("persist worker dropped flush ack".into()))?
    }

    pub async fn shutdown(&self) {
        let (ack_tx, ack_rx) = oneshot::channel();
        let _ = self.tx.send(Cmd::Shutdown { ack: ack_tx });
        let _ = ack_rx.await;
    }
}

async fn run_worker(store: ThreadStore, mut rx: mpsc::UnboundedReceiver<Cmd>) {
    let store = Arc::new(store);
    let mut pending: Option<Snapshot> = None;
    let mut deadline: Option<Instant> = None;
    let mut waiters: Vec<oneshot::Sender<Result<()>>> = Vec::new();
    let mut forgotten: HashSet<String> = HashSet::new();

    loop {
        let sleep_for = deadline.map(|d| {
            let now = Instant::now();
            if d <= now {
                Duration::from_millis(0)
            } else {
                d.saturating_duration_since(now)
            }
        });

        tokio::select! {
            biased;

            cmd = rx.recv() => {
                match cmd {
                    None => {
                        let _ = flush_pending(&store, &mut pending, &mut deadline, &mut waiters, &forgotten).await;
                        break;
                    }
                    Some(Cmd::Shutdown { ack }) => {
                        let _ = flush_pending(&store, &mut pending, &mut deadline, &mut waiters, &forgotten).await;
                        let _ = ack.send(());
                        break;
                    }
                    Some(Cmd::Flush { ack }) => {
                        let result = flush_pending(&store, &mut pending, &mut deadline, &mut waiters, &forgotten).await;
                        let _ = ack.send(result);
                    }
                    Some(Cmd::Forget { thread_id, ack }) => {
                        forgotten.insert(thread_id.clone());
                        if pending.as_ref().is_some_and(|snapshot| {
                            snapshot_thread_id(snapshot).as_deref() == Some(thread_id.as_str())
                        }) {
                            pending = None;
                            deadline = None;
                            for waiter in waiters.drain(..) {
                                let _ = waiter.send(Ok(()));
                            }
                        }
                        let _ = ack.send(Ok(()));
                    }
                    Some(Cmd::Upsert { snapshot, priority, ack }) => {
                        if is_forgotten(&forgotten, &snapshot) {
                            if let Some(ack) = ack {
                                let _ = ack.send(Ok(()));
                            }
                            continue;
                        }
                        match priority {
                            PersistPriority::Immediate => {
                                // Collapse any pending delta into this write.
                                pending = None;
                                deadline = None;
                                let result = write_off_runtime(&store, snapshot).await;
                                let for_waiters = clone_result(&result);
                                if let Some(ack) = ack {
                                    let _ = ack.send(result);
                                }
                                for w in waiters.drain(..) {
                                    let _ = w.send(clone_result(&for_waiters));
                                }
                            }
                            PersistPriority::Delta => {
                                pending = Some(snapshot);
                                if deadline.is_none() {
                                    deadline = Some(
                                        Instant::now()
                                            + Duration::from_millis(DELTA_CHECKPOINT_MS),
                                    );
                                }
                                if let Some(ack) = ack {
                                    waiters.push(ack);
                                }
                            }
                        }
                    }
                }
            }

            _ = async {
                if let Some(dur) = sleep_for {
                    tokio::time::sleep(dur).await;
                } else {
                    std::future::pending::<()>().await;
                }
            }, if deadline.is_some() => {
                let _ = flush_pending(&store, &mut pending, &mut deadline, &mut waiters, &forgotten).await;
            }
        }
    }
}

fn snapshot_thread_id(snapshot: &Snapshot) -> Option<String> {
    match snapshot {
        Snapshot::Owned(thread) => Some(thread.id.clone()),
        Snapshot::Live(live) => Some(
            live.lock()
                .unwrap_or_else(|error| error.into_inner())
                .id
                .clone(),
        ),
    }
}

fn is_forgotten(forgotten: &HashSet<String>, snapshot: &Snapshot) -> bool {
    snapshot_thread_id(snapshot).is_some_and(|thread_id| forgotten.contains(&thread_id))
}

async fn flush_pending(
    store: &Arc<ThreadStore>,
    pending: &mut Option<Snapshot>,
    deadline: &mut Option<Instant>,
    waiters: &mut Vec<oneshot::Sender<Result<()>>>,
    forgotten: &HashSet<String>,
) -> Result<()> {
    *deadline = None;
    let Some(snapshot) = pending.take() else {
        for w in waiters.drain(..) {
            let _ = w.send(Ok(()));
        }
        return Ok(());
    };
    if is_forgotten(forgotten, &snapshot) {
        for waiter in waiters.drain(..) {
            let _ = waiter.send(Ok(()));
        }
        return Ok(());
    }
    let result = write_off_runtime(store, snapshot).await;
    for w in waiters.drain(..) {
        let _ = w.send(clone_result(&result));
    }
    result
}

/// Serialize and write on a blocking thread.
///
/// [`crate::fsutil::atomic_write`] ends in `sync_all` — a real fsync — and the
/// document is the entire conversation. Doing that inline stalled a runtime
/// worker for the duration of a disk sync, several times a second while a turn
/// streams, which is exactly the kind of work Tokio asks you not to do on an
/// async thread.
async fn write_off_runtime(store: &Arc<ThreadStore>, snapshot: Snapshot) -> Result<()> {
    let store = store.clone();
    let task = tokio::task::spawn_blocking(move || match snapshot {
        Snapshot::Owned(thread) => store.save(&thread),
        Snapshot::Live(live) => {
            // Taken here, on the blocking thread, so a producer mid-event never
            // stalls a runtime worker. A poisoned lock still holds a usable
            // thread — losing the transcript because a different thread
            // panicked would be the worse outcome.
            let thread = live.lock().unwrap_or_else(|e| e.into_inner());
            store.save(&thread)
        }
    });
    match task.await {
        Ok(result) => result,
        Err(e) => Err(HarnessError::Other(format!("persist task failed: {e}"))),
    }
}

fn clone_result(result: &Result<()>) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(e) => Err(HarnessError::Other(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> crate::fsutil::ScratchDir {
        crate::fsutil::ScratchDir::new(&format!("zest-persist-{name}-"))
    }

    #[tokio::test]
    async fn immediate_save_round_trips() {
        let root = scratch("imm");
        let worker = PersistWorker::spawn(&root).unwrap();
        let mut thread = Thread::new();
        thread.apply_user("u1", "hello");
        worker
            .save_and_wait(Snapshot::owned(thread.clone()), PersistPriority::Immediate)
            .await
            .unwrap();
        let loaded = ThreadStore::open(&root).unwrap().load(&thread.id).unwrap();
        assert_eq!(loaded.messages.len(), 1);
        worker.shutdown().await;
    }

    #[tokio::test]
    async fn a_live_snapshot_is_read_when_it_is_written_not_when_it_is_queued() {
        // The point of the Live variant: the producer hands over a handle and
        // keeps appending. Whatever the thread says at flush time is what lands,
        // so a burst of deltas costs one copy instead of one per delta.
        let root = scratch("live");
        let worker = PersistWorker::spawn(&root).unwrap();

        let mut seed = Thread::new();
        seed.apply_user("u1", "hi");
        let id = seed.id.clone();
        let live = Arc::new(std::sync::Mutex::new(seed));

        worker
            .enqueue(Snapshot::Live(live.clone()), PersistPriority::Delta)
            .unwrap();

        // Appended *after* the enqueue, and still expected on disk.
        live.lock().unwrap().apply_text_delta("a1", "written later");

        worker.flush().await.unwrap();
        let loaded = ThreadStore::open(&root).unwrap().load(&id).unwrap();
        match &loaded.messages[1] {
            crate::thread::StoredMessage::Assistant { text, .. } => {
                assert_eq!(text, "written later");
            }
            other => panic!("expected assistant, got {other:?}"),
        }
        worker.shutdown().await;
    }

    #[tokio::test]
    async fn many_deltas_against_one_handle_still_write_once() {
        // Coalescing was always here; what changed is that the queued items are
        // now handles rather than whole transcripts.
        let root = scratch("live-burst");
        let worker = PersistWorker::spawn(&root).unwrap();

        let mut seed = Thread::new();
        seed.apply_user("u1", "hi");
        let id = seed.id.clone();
        let live = Arc::new(std::sync::Mutex::new(seed));

        for index in 0..200 {
            live.lock()
                .unwrap()
                .apply_text_delta("a1", &format!("{index} "));
            worker
                .enqueue(Snapshot::Live(live.clone()), PersistPriority::Delta)
                .unwrap();
        }

        worker.flush().await.unwrap();
        let loaded = ThreadStore::open(&root).unwrap().load(&id).unwrap();
        match &loaded.messages[1] {
            crate::thread::StoredMessage::Assistant { text, .. } => {
                assert!(text.starts_with("0 1 2 "), "{text}");
                assert!(text.trim_end().ends_with("199"), "{text}");
            }
            other => panic!("expected assistant, got {other:?}"),
        }
        worker.shutdown().await;
    }

    #[tokio::test]
    async fn delta_coalesces_until_flush() {
        let root = scratch("delta");
        let worker = PersistWorker::spawn(&root).unwrap();
        let mut thread = Thread::new();
        thread.apply_user("u1", "hi");
        worker
            .save_and_wait(Snapshot::owned(thread.clone()), PersistPriority::Immediate)
            .await
            .unwrap();

        thread.apply_text_delta("a1", "partial");
        worker
            .enqueue(Snapshot::owned(thread.clone()), PersistPriority::Delta)
            .unwrap();
        // Before checkpoint interval, disk may still lack the delta — flush forces it.
        worker.flush().await.unwrap();
        let loaded = ThreadStore::open(&root).unwrap().load(&thread.id).unwrap();
        match &loaded.messages[1] {
            crate::thread::StoredMessage::Assistant { text, .. } => {
                assert_eq!(text, "partial");
            }
            other => panic!("expected assistant, got {other:?}"),
        }
        worker.shutdown().await;
    }

    #[tokio::test]
    async fn forget_stops_later_writes_from_recreating_a_deleted_chat() {
        let root = scratch("forget");
        let worker = PersistWorker::spawn(&root).unwrap();
        let mut thread = Thread::new();
        thread.apply_user("u1", "hello");
        worker
            .save_and_wait(Snapshot::owned(thread.clone()), PersistPriority::Immediate)
            .await
            .unwrap();
        ThreadStore::open(&root)
            .unwrap()
            .delete(&thread.id)
            .unwrap();

        worker.forget(&thread.id).await.unwrap();
        thread.apply_text_delta("a1", "after delete");
        worker
            .save_and_wait(Snapshot::owned(thread.clone()), PersistPriority::Immediate)
            .await
            .unwrap();

        assert!(
            ThreadStore::open(&root).unwrap().load(&thread.id).is_err(),
            "a finishing turn must not recreate a deleted chat"
        );
        worker.shutdown().await;
    }
}
