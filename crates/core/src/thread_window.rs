//! Tail-first windows over a UI transcript.
//!
//! The window is the last N user turns: each user message plus the assistant
//! rows that follow it. Older history stays on disk until the UI asks for the
//! next page.

use super::thread::StoredMessage;

/// First paint: latest user turns, including their assistant follow-up.
pub const THREAD_WINDOW_USER_TURNS: usize = 10;
/// Scroll-up page. Larger than the first window so a second fetch covers more.
pub const THREAD_OLDER_USER_TURNS: usize = 20;

/// A slice of `messages` cut on user-turn boundaries.
#[derive(Debug, Clone)]
pub struct MessageWindow {
    pub messages: Vec<StoredMessage>,
    pub has_older: bool,
    pub has_newer: bool,
    pub hidden_user_turns: usize,
}

impl MessageWindow {
    /// Last `user_turns` user messages and everything after the first of those.
    pub fn tail(messages: &[StoredMessage], user_turns: usize) -> Self {
        let user_turns = user_turns.max(1);
        let starts = user_start_indices(messages);
        let start_turn = starts.len().saturating_sub(user_turns);
        Self::from_user_turns(messages, &starts, start_turn, user_turns)
    }

    /// A window of `user_turns` that contains `focus_id`.
    ///
    /// Prefers the tail when the hit already sits in those last turns, so a
    /// recent match still opens on the latest page.
    pub fn around(
        messages: &[StoredMessage],
        focus_id: &str,
        user_turns: usize,
    ) -> Result<Self, String> {
        let user_turns = user_turns.max(1);
        let starts = user_start_indices(messages);
        let focus = messages
            .iter()
            .position(|message| message.id() == focus_id)
            .ok_or_else(|| "that message is not in this chat".to_string())?;
        let focus_turn = starts
            .iter()
            .rposition(|&start| start <= focus)
            .unwrap_or(0);
        let start_turn = focus_turn.min(starts.len().saturating_sub(user_turns));
        Ok(Self::from_user_turns(
            messages, &starts, start_turn, user_turns,
        ))
    }

    /// The page of user turns that ends just before `before_id`.
    pub fn before(
        messages: &[StoredMessage],
        before_id: &str,
        user_turns: usize,
    ) -> Result<Self, String> {
        let end = messages
            .iter()
            .position(|message| message.id() == before_id)
            .ok_or_else(|| "that message is not in this chat".to_string())?;
        Ok(Self::tail(&messages[..end], user_turns))
    }

    /// The page of user turns that starts just after `after_id`.
    pub fn after(
        messages: &[StoredMessage],
        after_id: &str,
        user_turns: usize,
    ) -> Result<Self, String> {
        let pos = messages
            .iter()
            .position(|message| message.id() == after_id)
            .ok_or_else(|| "that message is not in this chat".to_string())?;
        let rest = &messages[pos + 1..];
        let skip = rest
            .iter()
            .position(|message| matches!(message, StoredMessage::User { .. }))
            .unwrap_or(rest.len());
        let start = pos + 1 + skip;
        let hidden_user_turns = user_start_indices(&messages[..start]).len();
        let mut page = Self::head(&messages[start..], user_turns);
        page.has_older = start > 0;
        page.hidden_user_turns = hidden_user_turns;
        Ok(page)
    }

    fn head(messages: &[StoredMessage], user_turns: usize) -> Self {
        let user_turns = user_turns.max(1);
        let starts = user_start_indices(messages);
        Self::from_user_turns(messages, &starts, 0, user_turns)
    }

    fn from_user_turns(
        messages: &[StoredMessage],
        starts: &[usize],
        start_turn: usize,
        user_turns: usize,
    ) -> Self {
        let user_turns = user_turns.max(1);
        if starts.is_empty() {
            return Self {
                messages: messages.to_vec(),
                has_older: false,
                has_newer: false,
                hidden_user_turns: 0,
            };
        }
        let start_turn = start_turn.min(starts.len().saturating_sub(1));
        let end_turn = (start_turn + user_turns).min(starts.len());
        let msg_start = if start_turn == 0 {
            0
        } else {
            starts[start_turn]
        };
        let msg_end = if end_turn == starts.len() {
            messages.len()
        } else {
            starts[end_turn]
        };
        Self {
            messages: messages[msg_start..msg_end].to_vec(),
            has_older: start_turn > 0,
            has_newer: end_turn < starts.len(),
            hidden_user_turns: start_turn,
        }
    }
}

fn user_start_indices(messages: &[StoredMessage]) -> Vec<usize> {
    messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| match message {
            StoredMessage::User { .. } => Some(index),
            StoredMessage::Assistant { .. } => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: &str, text: &str) -> StoredMessage {
        StoredMessage::User {
            id: id.into(),
            text: text.into(),
        }
    }

    fn assistant(id: &str, text: &str) -> StoredMessage {
        StoredMessage::Assistant {
            id: id.into(),
            text: text.into(),
            thinking: String::new(),
            tools: Vec::new(),
            error: None,
            provider_selection: None,
            command: None,
            streaming: false,
        }
    }

    fn ids(window: &MessageWindow) -> Vec<&str> {
        window.messages.iter().map(StoredMessage::id).collect()
    }

    fn thread_with_users(count: usize) -> Vec<StoredMessage> {
        let mut messages = Vec::new();
        for index in 1..=count {
            messages.push(user(&format!("u{index}"), &format!("ask {index}")));
            messages.push(assistant(&format!("a{index}"), &format!("ok {index}")));
        }
        messages
    }

    #[test]
    fn a_short_thread_is_returned_whole() {
        let messages = thread_with_users(3);
        let window = MessageWindow::tail(&messages, 10);
        assert_eq!(ids(&window), ["u1", "a1", "u2", "a2", "u3", "a3"]);
        assert!(!window.has_older);
        assert!(!window.has_newer);
        assert_eq!(window.hidden_user_turns, 0);
    }

    #[test]
    fn the_first_paint_keeps_the_last_user_turns() {
        let messages = thread_with_users(15);
        let window = MessageWindow::tail(&messages, 10);
        assert_eq!(window.messages[0].id(), "u6");
        assert_eq!(window.messages.last().map(StoredMessage::id), Some("a15"));
        assert_eq!(window.messages.len(), 20);
        assert!(window.has_older);
        assert!(!window.has_newer);
        assert_eq!(window.hidden_user_turns, 5);
    }

    #[test]
    fn assistant_rows_ride_with_their_user_turn() {
        let messages = vec![
            user("u1", "one"),
            assistant("a1", "fan-out"),
            assistant("a2", "more"),
            assistant("a3", "still"),
            user("u2", "two"),
            assistant("a4", "done"),
        ];
        let window = MessageWindow::tail(&messages, 1);
        assert_eq!(ids(&window), ["u2", "a4"]);
        assert!(window.has_older);
        assert!(!window.has_newer);
        assert_eq!(window.hidden_user_turns, 1);
    }

    #[test]
    fn older_pages_are_disjoint_and_cover_the_rest() {
        let messages = thread_with_users(15);
        let first = MessageWindow::tail(&messages, 10);
        let older = MessageWindow::before(&messages, first.messages[0].id(), 20).unwrap();
        assert_eq!(older.messages[0].id(), "u1");
        assert_eq!(older.messages.last().map(StoredMessage::id), Some("a5"));
        assert!(!older.has_older);
        assert!(!older.has_newer);
        assert_eq!(older.hidden_user_turns, 0);

        let first_ids: Vec<&str> = ids(&first);
        for id in ids(&older) {
            assert!(!first_ids.contains(&id), "{id} appeared in both pages");
        }
        let walked: Vec<&str> = ids(&older).into_iter().chain(ids(&first)).collect();
        let all: Vec<&str> = messages.iter().map(StoredMessage::id).collect();
        assert_eq!(walked, all);
    }

    #[test]
    fn a_saved_thread_opens_on_the_latest_user_turns() {
        let mut thread = crate::thread::Thread::new();
        for index in 1..=15 {
            thread.apply_user(&format!("u{index}"), &format!("Turn {index} prompt"));
            thread.apply_assistant_start(&format!("a{index}"), None);
            thread.apply_text_delta(&format!("a{index}"), &format!("Turn {index} reply"));
        }
        let window = MessageWindow::tail(&thread.messages, THREAD_WINDOW_USER_TURNS);
        assert_eq!(window.messages[0].id(), "u6");
        assert_eq!(window.messages.last().map(StoredMessage::id), Some("a15"));
        assert!(window.has_older);
        assert!(!window.has_newer);
        assert_eq!(window.hidden_user_turns, 5);
    }

    #[test]
    fn a_search_hit_near_the_start_opens_on_that_turn() {
        let messages = thread_with_users(15);
        let window = MessageWindow::around(&messages, "u1", 10).unwrap();
        assert_eq!(window.messages[0].id(), "u1");
        assert_eq!(window.messages.last().map(StoredMessage::id), Some("a10"));
        assert!(!window.has_older);
        assert!(window.has_newer);
        assert_eq!(window.hidden_user_turns, 0);

        let newer = MessageWindow::after(&messages, "a10", 20).unwrap();
        assert_eq!(newer.messages[0].id(), "u11");
        assert_eq!(newer.messages.last().map(StoredMessage::id), Some("a15"));
        assert!(newer.has_older);
        assert!(!newer.has_newer);
        assert_eq!(newer.hidden_user_turns, 10);
        let first_ids: Vec<&str> = ids(&window);
        for id in ids(&newer) {
            assert!(!first_ids.contains(&id), "{id} appeared in both pages");
        }
    }

    #[test]
    fn a_search_hit_in_the_tail_opens_on_the_latest_page() {
        let messages = thread_with_users(15);
        let window = MessageWindow::around(&messages, "u12", 10).unwrap();
        assert_eq!(window.messages[0].id(), "u6");
        assert_eq!(window.messages.last().map(StoredMessage::id), Some("a15"));
        assert!(window.has_older);
        assert!(!window.has_newer);
    }

    #[test]
    fn a_missing_cursor_is_an_error() {
        let messages = thread_with_users(2);
        let err = MessageWindow::before(&messages, "nope", 10).unwrap_err();
        assert!(err.contains("not in this chat"), "{err}");
        let err = MessageWindow::around(&messages, "nope", 10).unwrap_err();
        assert!(err.contains("not in this chat"), "{err}");
    }

    #[test]
    fn leading_assistants_land_on_the_oldest_page() {
        let mut messages = vec![assistant("pre", "hello")];
        messages.extend(thread_with_users(11));
        let first = MessageWindow::tail(&messages, 10);
        assert_eq!(first.messages[0].id(), "u2");
        let older = MessageWindow::before(&messages, "u2", 20).unwrap();
        assert_eq!(ids(&older)[0], "pre");
        assert!(ids(&older).contains(&"u1"));
        assert!(!older.has_older);
    }
}
