//! Session controller: monotonic identities, chat-scoped turns, and cancel.
//!
//! A turn belongs to the chat that started it, not to the window's current
//! route. Idle sessions can be replaced when the user navigates, while a
//! running session stays registered until its worker quiesces. That lets the
//! desktop continue a response in the background without allowing a stale turn
//! to restore over a newer chat.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

#[cfg(test)]
use zest_core::ThreadInputTarget;
use zest_core::{
    new_id, Agent, CancelToken, InputInbox, RecoverableRun, SkillSet, Thread, ThreadInput,
};

use super::{ApprovalHub, QuestionHub};

pub struct Session {
    pub session_id: String,
    pub agent: Agent,
    pub model: String,
    pub effort: String,
    pub provider_id: String,
    pub provider_label: String,
    pub root: PathBuf,
    pub thread_id: String,
    pub thread: Thread,
    /// A previous process left a provider turn unfinished. The prompt remains
    /// in `thread.messages`; this identity lets the UI offer a fresh retry
    /// without claiming the provider can resume its old stream.
    pub recovery: Option<RecoverableRun>,
    /// Front-end base prompt (before custom + skills layers).
    pub base_system: String,
    /// Shared with `read_skill` for hot-reload.
    pub skills: Arc<RwLock<SkillSet>>,
    /// Approval/question waiters are owned by this chat so parallel turns do
    /// not resolve or clear one another's prompts.
    pub(crate) approval_hub: Arc<ApprovalHub>,
    pub(crate) question_hub: Arc<QuestionHub>,
}

#[derive(Clone)]
pub struct ActiveTurn {
    pub turn_id: String,
    pub session_id: String,
    pub thread_id: String,
    pub root: PathBuf,
    pub provider_id: String,
    pub provider_label: String,
    pub model: String,
    pub effort: String,
    pub cancel: CancelToken,
    /// Live transcript shared with queue commands while the session body is in
    /// the turn worker. This prevents a queue write from racing a streaming
    /// snapshot and overwriting newer deltas.
    pub(crate) live_thread: Arc<Mutex<Thread>>,
    /// Runtime projection of the same durable queue for steer/inject delivery.
    pub(crate) input_inbox: Arc<InputInbox>,
    pub(crate) approval_hub: Arc<ApprovalHub>,
    pub(crate) question_hub: Arc<QuestionHub>,
}

struct SessionSlot {
    /// Idle body. It is taken while a worker owns the turn and restored by
    /// `finish_turn` if this slot has not been ended.
    session: Option<Session>,
    turn: Option<ActiveTurn>,
    ended: bool,
}

struct Inner {
    next_seq: u64,
    /// Session currently shown by the desktop route.
    active_session_id: Option<String>,
    /// Idle and in-flight chats. A running chat remains here after navigation
    /// so its turn can finish and restore its agent state safely.
    sessions: HashMap<String, SessionSlot>,
}

pub struct SessionController {
    inner: Mutex<Inner>,
}

#[derive(Debug)]
pub enum SessionError {
    Busy,
    NoSession,
    Poisoned,
}

impl SessionError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::NoSession => "no_session",
            Self::Poisoned => "poisoned",
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            // Reaches the UI verbatim, so it has to say what to do rather than
            // describe internal state.
            Self::Busy => "this chat is still working — switch chats or wait for it to finish",
            Self::NoSession => "no active session — choose a provider first",
            Self::Poisoned => "session lock poisoned",
        }
    }
}

impl SessionController {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                next_seq: 1,
                active_session_id: None,
                sessions: HashMap::new(),
            }),
        }
    }

    pub fn is_busy(&self) -> Result<bool, SessionError> {
        let g = self.inner.lock().map_err(|_| SessionError::Poisoned)?;
        Ok(g.sessions.values().any(|slot| slot.turn.is_some()))
    }

    pub fn require_idle(&self) -> Result<(), SessionError> {
        let g = self.inner.lock().map_err(|_| SessionError::Poisoned)?;
        let id = g
            .active_session_id
            .as_ref()
            .ok_or(SessionError::NoSession)?;
        let slot = g.sessions.get(id).ok_or(SessionError::NoSession)?;
        if slot.turn.is_some() {
            Err(SessionError::Busy)
        } else if slot.session.is_some() {
            Ok(())
        } else {
            Err(SessionError::NoSession)
        }
    }

    pub fn set_session(&self, mut session: Session) -> Result<(), SessionError> {
        let mut g = self.inner.lock().map_err(|_| SessionError::Poisoned)?;
        // An idle route no longer needs to stay resident once another chat is
        // opened. In-flight routes are retained until their worker finishes.
        if let Some(previous_id) = g.active_session_id.take() {
            let remove_previous = g
                .sessions
                .get(&previous_id)
                .is_some_and(|slot| slot.turn.is_none());
            if remove_previous {
                g.sessions.remove(&previous_id);
            } else {
                g.active_session_id = Some(previous_id);
            }
        }
        let seq = g.next_seq;
        g.next_seq = g.next_seq.saturating_add(1);
        let session_id = format!("session-{seq}");
        session.session_id = session_id.clone();
        g.sessions.insert(
            session_id.clone(),
            SessionSlot {
                session: Some(session),
                turn: None,
                ended: false,
            },
        );
        g.active_session_id = Some(session_id);
        Ok(())
    }

    fn active_id(g: &Inner) -> Result<String, SessionError> {
        g.active_session_id.clone().ok_or(SessionError::NoSession)
    }

    fn active_slot_mut(g: &mut Inner) -> Result<&mut SessionSlot, SessionError> {
        let id = Self::active_id(g)?;
        g.sessions.get_mut(&id).ok_or(SessionError::NoSession)
    }

    fn active_slot(g: &Inner) -> Result<&SessionSlot, SessionError> {
        let id = Self::active_id(g)?;
        g.sessions.get(&id).ok_or(SessionError::NoSession)
    }

    pub fn with_session_mut<R>(
        &self,
        f: impl FnOnce(&mut Session) -> R,
    ) -> Result<R, SessionError> {
        let mut g = self.inner.lock().map_err(|_| SessionError::Poisoned)?;
        let slot = Self::active_slot_mut(&mut g)?;
        if slot.turn.is_some() {
            return Err(SessionError::Busy);
        }
        let session = slot.session.as_mut().ok_or(SessionError::NoSession)?;
        Ok(f(session))
    }

    /// Mutate the visible session only when it is idle. A running turn keeps
    /// the session body; metadata updates then apply to disk alone.
    pub fn with_session_if_idle<R>(
        &self,
        f: impl FnOnce(&mut Session) -> R,
    ) -> Result<Option<R>, SessionError> {
        let mut g = self.inner.lock().map_err(|_| SessionError::Poisoned)?;
        let slot = match Self::active_slot_mut(&mut g) {
            Ok(slot) => slot,
            Err(SessionError::NoSession) => return Ok(None),
            Err(error) => return Err(error),
        };
        if slot.turn.is_some() {
            return Ok(None);
        }
        let Some(session) = slot.session.as_mut() else {
            return Ok(None);
        };
        Ok(Some(f(session)))
    }

    /// Mutate a durable chat by id while it is idle, even when that chat is
    /// not the route currently visible in the window. Background completion
    /// notices use this to update the owning thread without stealing focus.
    pub fn with_thread_if_idle<R>(
        &self,
        thread_id: &str,
        f: impl FnOnce(&mut Session) -> R,
    ) -> Result<Option<R>, SessionError> {
        let mut g = self.inner.lock().map_err(|_| SessionError::Poisoned)?;
        let slot = g.sessions.values_mut().find(|slot| {
            slot.turn.is_none()
                && slot
                    .session
                    .as_ref()
                    .is_some_and(|session| session.thread_id == thread_id)
        });
        let Some(slot) = slot else {
            return Ok(None);
        };
        let Some(session) = slot.session.as_mut() else {
            return Ok(None);
        };
        Ok(Some(f(session)))
    }

    /// Durable chat id for the visible route, including while its body is
    /// held by a turn.
    pub fn active_thread_id(&self) -> Result<Option<String>, SessionError> {
        let g = self.inner.lock().map_err(|_| SessionError::Poisoned)?;
        let slot = match Self::active_slot(&g) {
            Ok(slot) => slot,
            Err(SessionError::NoSession) => return Ok(None),
            Err(error) => return Err(error),
        };
        if let Some(turn) = &slot.turn {
            return Ok(Some(turn.thread_id.clone()));
        }
        Ok(slot
            .session
            .as_ref()
            .map(|session| session.thread_id.clone()))
    }

    pub fn session_info_snapshot<R>(
        &self,
        f: impl FnOnce(&Session) -> R,
    ) -> Result<Option<R>, SessionError> {
        let g = self.inner.lock().map_err(|_| SessionError::Poisoned)?;
        let slot = match Self::active_slot(&g) {
            Ok(slot) => slot,
            Err(SessionError::NoSession) => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(slot.session.as_ref().map(f))
    }

    /// Root for the active route, including while its session body is held by
    /// a background turn.
    pub fn active_root(&self) -> Result<Option<PathBuf>, SessionError> {
        let g = self.inner.lock().map_err(|_| SessionError::Poisoned)?;
        let slot = match Self::active_slot(&g) {
            Ok(slot) => slot,
            Err(SessionError::NoSession) => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(slot
            .session
            .as_ref()
            .map(|session| session.root.clone())
            .or_else(|| slot.turn.as_ref().map(|turn| turn.root.clone())))
    }

    pub fn has_active_session(&self) -> Result<bool, SessionError> {
        let g = self.inner.lock().map_err(|_| SessionError::Poisoned)?;
        Ok(Self::active_slot(&g).is_ok())
    }

    /// Take the session for a turn. Records active turn metadata for cancel.
    pub fn begin_turn(&self) -> Result<(Session, ActiveTurn), SessionError> {
        let mut g = self.inner.lock().map_err(|_| SessionError::Poisoned)?;
        let active_id = Self::active_id(&g)?;
        let target_thread = g
            .sessions
            .get(&active_id)
            .and_then(|slot| slot.session.as_ref())
            .map(|session| session.thread_id.clone())
            .ok_or_else(|| {
                if g.sessions
                    .get(&active_id)
                    .is_some_and(|slot| slot.turn.is_some())
                {
                    SessionError::Busy
                } else {
                    SessionError::NoSession
                }
            })?;
        // Reopening a chat while its old runtime is still finishing creates a
        // second idle session object, but it must not start a second turn on
        // the same transcript.
        if g.sessions.values().any(|slot| {
            slot.turn
                .as_ref()
                .is_some_and(|turn| turn.thread_id == target_thread)
        }) {
            return Err(SessionError::Busy);
        }
        let slot = g
            .sessions
            .get_mut(&active_id)
            .ok_or(SessionError::NoSession)?;
        if slot.turn.is_some() {
            return Err(SessionError::Busy);
        }
        let session = slot.session.take().ok_or(SessionError::NoSession)?;
        let turn = ActiveTurn {
            turn_id: new_id("turn"),
            session_id: session.session_id.clone(),
            thread_id: session.thread_id.clone(),
            root: session.root.clone(),
            provider_id: session.provider_id.clone(),
            provider_label: session.provider_label.clone(),
            model: session.model.clone(),
            effort: session.effort.clone(),
            cancel: CancelToken::new(),
            live_thread: Arc::new(Mutex::new(session.thread.clone())),
            input_inbox: Arc::new(InputInbox::from_pending(
                session.thread.pending_inputs.clone(),
            )),
            approval_hub: session.approval_hub.clone(),
            question_hub: session.question_hub.clone(),
        };
        slot.turn = Some(turn.clone());
        Ok((session, turn))
    }

    /// Cancel the active turn token. Does not clear the turn slot — that waits
    /// for [`Self::finish_turn`] (quiesce).
    pub fn cancel_turn(&self) -> Result<bool, SessionError> {
        let g = self.inner.lock().map_err(|_| SessionError::Poisoned)?;
        let slot = Self::active_slot(&g)?;
        if let Some(turn) = &slot.turn {
            turn.cancel.cancel();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Snapshot the active turn for lifecycle persistence and UI commands that
    /// need the project root while the session body is temporarily in flight.
    pub fn active_turn(&self) -> Result<Option<ActiveTurn>, SessionError> {
        let g = self.inner.lock().map_err(|_| SessionError::Poisoned)?;
        let slot = match Self::active_slot(&g) {
            Ok(slot) => slot,
            Err(SessionError::NoSession) => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(slot.turn.clone())
    }

    /// Find a running turn by its durable chat id, even if the user has
    /// reopened that chat into a newer idle route.
    pub fn active_turn_for_thread(
        &self,
        thread_id: &str,
    ) -> Result<Option<ActiveTurn>, SessionError> {
        let g = self.inner.lock().map_err(|_| SessionError::Poisoned)?;
        Ok(g.sessions.values().find_map(|slot| {
            slot.turn
                .as_ref()
                .filter(|turn| turn.thread_id == thread_id)
                .cloned()
        }))
    }

    pub fn cancel_turn_for_thread(&self, thread_id: &str) -> Result<bool, SessionError> {
        let g = self.inner.lock().map_err(|_| SessionError::Poisoned)?;
        let Some(turn) = g.sessions.values().find_map(|slot| {
            slot.turn
                .as_ref()
                .filter(|turn| turn.thread_id == thread_id)
        }) else {
            return Ok(false);
        };
        turn.cancel.cancel();
        Ok(true)
    }

    /// Claim one followup after an active turn has restored its session body.
    /// The returned snapshot is what the caller persists before starting the
    /// next turn, making the claim crash-safe.
    pub fn take_followup_for_thread(
        &self,
        thread_id: &str,
    ) -> Result<Option<(ThreadInput, PathBuf, Thread)>, SessionError> {
        let mut g = self.inner.lock().map_err(|_| SessionError::Poisoned)?;
        let slot = g.sessions.values_mut().find(|slot| {
            slot.turn
                .as_ref()
                .is_some_and(|turn| turn.thread_id == thread_id)
                || slot
                    .session
                    .as_ref()
                    .is_some_and(|session| session.thread_id == thread_id)
        });
        let Some(slot) = slot else {
            return Ok(None);
        };
        if slot.turn.is_some() {
            return Ok(None);
        }
        let Some(session) = slot.session.as_mut() else {
            return Ok(None);
        };
        let Some(input) = session.thread.claim_followup() else {
            return Ok(None);
        };
        Ok(Some((input, session.root.clone(), session.thread.clone())))
    }

    /// Cancel a thread's turn and mark its slot ended so [`Self::finish_turn`]
    /// will not restore that transcript. Used when the user deletes a chat
    /// that is still working or waiting for approval.
    pub fn abandon_turn_for_thread(
        &self,
        thread_id: &str,
    ) -> Result<Option<ActiveTurn>, SessionError> {
        let mut g = self.inner.lock().map_err(|_| SessionError::Poisoned)?;
        let session_id = g.sessions.iter().find_map(|(id, slot)| {
            slot.turn
                .as_ref()
                .filter(|turn| turn.thread_id == thread_id)
                .map(|_| id.clone())
        });
        let Some(session_id) = session_id else {
            return Ok(None);
        };
        let slot = g
            .sessions
            .get_mut(&session_id)
            .ok_or(SessionError::NoSession)?;
        let Some(turn) = slot.turn.as_ref() else {
            return Ok(None);
        };
        turn.cancel.cancel();
        slot.ended = true;
        Ok(Some(turn.clone()))
    }

    /// Return the session after a turn. No-ops when the live session changed
    /// (end/start) so a stale turn cannot overwrite newer state.
    pub fn finish_turn(&self, turn: &ActiveTurn, session: Session) -> Result<bool, SessionError> {
        let mut g = self.inner.lock().map_err(|_| SessionError::Poisoned)?;
        let (restore, ended) = {
            let Some(slot) = g.sessions.get_mut(&turn.session_id) else {
                return Ok(false);
            };
            if !slot
                .turn
                .as_ref()
                .is_some_and(|active| active.turn_id == turn.turn_id)
            {
                return Ok(false);
            }
            let ended = slot.ended;
            let restore = !ended && slot.session.is_none();
            slot.turn = None;
            (restore, ended)
        };
        if ended {
            g.sessions.remove(&turn.session_id);
            return Ok(false);
        }
        if !restore {
            return Ok(false);
        }

        // If the user reopened this same chat while it was running, move the
        // finished runtime into that visible idle slot. This keeps its agent
        // history current instead of letting a duplicate route send from a
        // stale transcript after the background turn completes.
        let replacement_id = g.active_session_id.clone().filter(|id| {
            id != &turn.session_id
                && g.sessions.get(id).is_some_and(|candidate| {
                    candidate.turn.is_none()
                        && candidate
                            .session
                            .as_ref()
                            .is_some_and(|active| active.thread_id == turn.thread_id)
                })
        });
        if let Some(replacement_id) = replacement_id {
            let mut session = session;
            session.session_id = replacement_id.clone();
            if let Some(replacement) = g.sessions.get_mut(&replacement_id) {
                replacement.session = Some(session);
                replacement.ended = false;
            }
            g.sessions.remove(&turn.session_id);
        } else if let Some(slot) = g.sessions.get_mut(&turn.session_id) {
            slot.session = Some(session);
        }
        Ok(true)
    }

    pub fn end_session(&self) -> Result<(), SessionError> {
        let mut g = self.inner.lock().map_err(|_| SessionError::Poisoned)?;
        let Some(id) = g.active_session_id.take() else {
            return Ok(());
        };
        let remove = if let Some(slot) = g.sessions.get_mut(&id) {
            if let Some(turn) = &slot.turn {
                turn.cancel.cancel();
                slot.ended = true;
                false
            } else {
                true
            }
        } else {
            false
        };
        if remove {
            g.sessions.remove(&id);
        }
        Ok(())
    }
}

impl Default for SessionController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
pub(crate) fn test_session(thread_id: impl Into<String>, root: impl Into<PathBuf>) -> Session {
    use async_trait::async_trait;
    use zest_core::HarnessError;
    use zest_core::{AuthStatus, Completion, StreamEvent, TurnRequest};
    use zest_core::{Provider, ToolRegistry};

    struct StubProvider;

    #[async_trait]
    impl Provider for StubProvider {
        fn id(&self) -> &str {
            "stub"
        }
        fn default_model(&self) -> &str {
            "stub"
        }
        fn auth_status(&self) -> AuthStatus {
            AuthStatus::Ready { account: None }
        }
        async fn stream_turn(
            &self,
            _req: &TurnRequest,
            _on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
        ) -> zest_core::Result<Completion> {
            Err(HarnessError::Other("unused".into()))
        }
    }

    use std::sync::RwLock;
    use zest_core::SkillSet;
    let provider: Arc<dyn Provider> = Arc::new(StubProvider);
    Session {
        session_id: String::new(),
        agent: Agent::new(provider, ToolRegistry::new()),
        model: "m".into(),
        effort: "high".into(),
        provider_id: "stub".into(),
        provider_label: "Stub".into(),
        root: root.into(),
        thread_id: thread_id.into(),
        thread: Thread::new(),
        recovery: None,
        base_system: "test".into(),
        skills: Arc::new(RwLock::new(SkillSet::default())),
        approval_hub: Arc::new(ApprovalHub::new()),
        question_hub: Arc::new(QuestionHub::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_session(id_suffix: &str) -> Session {
        super::test_session(format!("thread-{id_suffix}"), PathBuf::from("."))
    }

    #[test]
    fn finish_turn_does_not_restore_after_end_session() {
        let ctl = SessionController::new();
        ctl.set_session(dummy_session("a")).unwrap();
        let (session, turn) = ctl.begin_turn().unwrap();
        ctl.end_session().unwrap();
        // Turn stays registered until quiesce.
        assert!(ctl.is_busy().unwrap());
        let restored = ctl.finish_turn(&turn, session).unwrap();
        assert!(!restored);
        assert!(!ctl.is_busy().unwrap());
    }

    #[test]
    fn cancel_sets_token() {
        let ctl = SessionController::new();
        ctl.set_session(dummy_session("b")).unwrap();
        let (_session, turn) = ctl.begin_turn().unwrap();
        assert!(ctl.cancel_turn().unwrap());
        assert!(turn.cancel.is_cancelled());
    }

    #[test]
    fn queued_followup_survives_finish_until_the_next_turn_claims_it() {
        let ctl = SessionController::new();
        ctl.set_session(dummy_session("queued")).unwrap();
        let (mut session, turn) = ctl.begin_turn().unwrap();
        let queued = turn
            .live_thread
            .lock()
            .unwrap()
            .enqueue_input(ThreadInputTarget::Followup, "continue", Vec::new())
            .unwrap();
        turn.input_inbox.enqueue(queued.clone());
        session.thread = turn.live_thread.lock().unwrap().clone();

        assert!(ctl.finish_turn(&turn, session).unwrap());
        let (claimed, root, snapshot) = ctl
            .take_followup_for_thread("thread-queued")
            .unwrap()
            .expect("followup remains after the first turn");
        assert_eq!(claimed.id, queued.id);
        assert_eq!(claimed.text, "continue");
        assert_eq!(root, PathBuf::from("."));
        assert!(snapshot.pending_inputs.is_empty());
        assert!(snapshot.events.iter().any(|entry| matches!(
            entry.event,
            zest_core::ThreadEventKind::InputClaimed { ref input_id, .. }
                if input_id == &queued.id
        )));
    }

    #[test]
    fn cancellation_restores_the_session_without_discarding_queued_followups() {
        let ctl = SessionController::new();
        ctl.set_session(dummy_session("cancel-queue")).unwrap();
        let (mut session, turn) = ctl.begin_turn().unwrap();
        session
            .thread
            .enqueue_input(
                ThreadInputTarget::Followup,
                "retry after cancel",
                Vec::new(),
            )
            .unwrap();
        assert!(ctl.cancel_turn().unwrap());
        assert!(turn.cancel.is_cancelled());
        assert!(ctl.finish_turn(&turn, session).unwrap());

        let pending = ctl
            .session_info_snapshot(|session| session.thread.pending_inputs.clone())
            .unwrap()
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].text, "retry after cancel");
    }

    #[test]
    fn background_turn_survives_navigation_to_another_chat() {
        let ctl = SessionController::new();
        ctl.set_session(dummy_session("a")).unwrap();
        let (first, first_turn) = ctl.begin_turn().unwrap();

        ctl.set_session(dummy_session("b")).unwrap();
        let (second, second_turn) = ctl.begin_turn().unwrap();

        assert!(ctl.active_turn_for_thread("thread-a").unwrap().is_some());
        assert!(ctl.cancel_turn_for_thread("thread-a").unwrap());
        assert!(first_turn.cancel.is_cancelled());
        assert!(ctl.finish_turn(&first_turn, first).unwrap());
        assert!(ctl.finish_turn(&second_turn, second).unwrap());
        assert!(!ctl.is_busy().unwrap());
    }

    #[test]
    fn finishing_a_background_turn_refreshes_a_reopened_chat_slot() {
        let ctl = SessionController::new();
        ctl.set_session(dummy_session("a")).unwrap();
        let (first, first_turn) = ctl.begin_turn().unwrap();

        // This is what navigation does when the user returns to a chat whose
        // original runtime is still streaming.
        ctl.set_session(dummy_session("a")).unwrap();
        assert!(ctl.finish_turn(&first_turn, first).unwrap());
        let (reopened, _) = ctl.begin_turn().unwrap();
        assert_eq!(reopened.thread_id, "thread-a");
    }

    #[test]
    fn abandoning_a_turn_cancels_it_and_finish_does_not_restore() {
        let ctl = SessionController::new();
        ctl.set_session(dummy_session("a")).unwrap();
        let (session, turn) = ctl.begin_turn().unwrap();
        assert_eq!(ctl.active_thread_id().unwrap().as_deref(), Some("thread-a"));

        let abandoned = ctl.abandon_turn_for_thread("thread-a").unwrap();
        assert!(abandoned.is_some());
        assert!(turn.cancel.is_cancelled());
        assert!(ctl.with_session_if_idle(|_| ()).unwrap().is_none());

        ctl.set_session(dummy_session("b")).unwrap();
        assert_eq!(ctl.active_thread_id().unwrap().as_deref(), Some("thread-b"));
        assert!(!ctl.finish_turn(&turn, session).unwrap());
        assert!(ctl.active_turn_for_thread("thread-a").unwrap().is_none());
        assert_eq!(ctl.active_thread_id().unwrap().as_deref(), Some("thread-b"));
    }

    #[test]
    fn deleting_a_background_chat_does_not_block_the_visible_route() {
        let ctl = SessionController::new();
        ctl.set_session(dummy_session("a")).unwrap();
        let (first, first_turn) = ctl.begin_turn().unwrap();

        ctl.set_session(dummy_session("b")).unwrap();
        assert!(ctl
            .with_session_if_idle(|session| session.thread_id.clone())
            .unwrap()
            .is_some());

        assert!(ctl.abandon_turn_for_thread("thread-a").unwrap().is_some());
        assert!(first_turn.cancel.is_cancelled());
        assert!(!ctl.finish_turn(&first_turn, first).unwrap());
        assert_eq!(ctl.active_thread_id().unwrap().as_deref(), Some("thread-b"));
    }
}
