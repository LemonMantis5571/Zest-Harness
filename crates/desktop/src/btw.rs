//! Side-chat IPC deliberately bypasses the main turn queue and persistence.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tauri::{ipc::Channel, State};
use zest_core::{btw::SideConversation, new_id, CancelToken, StreamEvent};

use super::{map_session_err, AppState};

#[derive(Default)]
pub(crate) struct SideConversations(Mutex<HashMap<String, Arc<SideSlot>>>);

struct SideSlot {
    owner: String,
    conversation: tokio::sync::Mutex<SideConversation>,
    control: Mutex<Control>,
}

#[derive(Default)]
struct Control {
    closed: bool,
    cancel: Option<CancelToken>,
}

impl SideSlot {
    fn close(&self) {
        if let Ok(mut control) = self.control.lock() {
            control.closed = true;
            if let Some(cancel) = &control.cancel {
                cancel.cancel();
            }
        }
    }
}

impl SideConversations {
    pub(crate) fn close_owner(&self, owner: &str) {
        if let Ok(mut slots) = self.0.lock() {
            slots.retain(|_, slot| {
                if slot.owner == owner {
                    slot.close();
                    false
                } else {
                    true
                }
            });
        }
    }

    fn open(&self, owner: String, conversation: SideConversation) -> Result<String, String> {
        let mut slots = self
            .0
            .lock()
            .map_err(|_| "Side conversations are unavailable.")?;
        // One ephemeral branch per visible session, bounded if a webview goes
        // away without delivering its cleanup IPC.
        slots.retain(|_, slot| {
            if slot.owner == owner {
                slot.close();
                false
            } else {
                true
            }
        });
        if slots.len() >= 8 {
            return Err("Close another side conversation before opening one.".into());
        }
        let id = new_id("btw");
        slots.insert(
            id.clone(),
            Arc::new(SideSlot {
                owner,
                conversation: tokio::sync::Mutex::new(conversation),
                control: Mutex::new(Control::default()),
            }),
        );
        Ok(id)
    }

    fn get(&self, id: &str) -> Result<Arc<SideSlot>, String> {
        self.0
            .lock()
            .map_err(|_| "Side conversations are unavailable.")?
            .get(id)
            .cloned()
            .ok_or_else(|| "This side conversation is closed. Open /btw again.".into())
    }
}

#[tauri::command]
pub(crate) fn start_btw(state: State<'_, AppState>, session_id: String) -> Result<String, String> {
    let context = state
        .sessions
        .side_context(&session_id)
        .map_err(map_session_err)?;
    state.sessions.side_conversations.open(session_id, context)
}

#[tauri::command]
pub(crate) async fn send_btw(
    state: State<'_, AppState>,
    id: String,
    text: String,
    on_delta: Channel<String>,
) -> Result<String, String> {
    let slot = state.sessions.side_conversations.get(&id)?;
    let mut conversation = slot
        .conversation
        .try_lock()
        .map_err(|_| "This side conversation is still answering.")?;
    let cancel = CancelToken::new();
    {
        let mut control = slot
            .control
            .lock()
            .map_err(|_| "Side conversation is unavailable.")?;
        if control.closed {
            return Err("This side conversation is closed.".into());
        }
        control.cancel = Some(cancel.clone());
    }
    let result = conversation
        .send(&text, &cancel, &mut |event| {
            if let StreamEvent::Text(text) = event {
                if on_delta.send(text.to_string()).is_err() {
                    cancel.cancel();
                }
            }
        })
        .await;
    if let Ok(mut control) = slot.control.lock() {
        control.cancel = None;
    }
    result.map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn cancel_btw(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let slot = state.sessions.side_conversations.get(&id)?;
    let control = slot
        .control
        .lock()
        .map_err(|_| "Side conversation is unavailable.")?;
    if let Some(cancel) = &control.cancel {
        cancel.cancel();
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn close_btw(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let slot = state
        .sessions
        .side_conversations
        .0
        .lock()
        .map_err(|_| "Side conversations are unavailable.")?
        .remove(&id);
    if let Some(slot) = slot {
        slot.close();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::session::SessionController;
    use super::*;

    #[test]
    fn btw_close_and_reopen_cancel_only_the_side_answer() {
        let controller = SessionController::new();
        controller
            .set_session(super::super::session::test_session("main", "."))
            .unwrap();
        let (session, turn) = controller.begin_turn().unwrap();
        let context = controller.side_context(&turn.session_id).unwrap();
        let id = controller
            .side_conversations
            .open(turn.session_id.clone(), context)
            .unwrap();
        let slot = controller.side_conversations.get(&id).unwrap();
        let cancel = CancelToken::new();
        slot.control.lock().unwrap().cancel = Some(cancel.clone());
        let replacement = controller
            .side_conversations
            .open(
                turn.session_id.clone(),
                controller.side_context(&turn.session_id).unwrap(),
            )
            .unwrap();
        assert!(cancel.is_cancelled());
        assert!(!turn.cancel.is_cancelled());
        assert!(controller.side_conversations.get(&id).is_err());
        assert!(controller.side_conversations.get(&replacement).is_ok());
        assert!(controller.side_context("stale-session").is_err());
        assert!(controller.finish_turn(&turn, session).unwrap());
        controller
            .set_session(super::super::session::test_session("other", "."))
            .unwrap();
        assert!(controller.side_conversations.get(&replacement).is_err());
    }
}
