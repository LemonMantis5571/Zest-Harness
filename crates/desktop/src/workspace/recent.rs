//! The sidebar's Recent section: chats started outside a project, kept in
//! `<config dir>/zest/free-chats` (earlier versions of the app could start them). A window
//! holds one project, so opening one opens (or focuses) a window rooted at that folder.

use super::*;
use crate::theme::{self, Icon};
use crate::ui;

/// Recent lists at most this many, newest first.
const MAX_RECENT: usize = 30;

pub(super) fn free_chats_root() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("zest").join("free-chats"))
}

/// Free chats with messages, newest first, as `list_cached_threads` returns them.
pub(super) fn list_free_chats(root: &std::path::Path) -> Vec<ThreadSummary> {
    if !root.join(".zest").is_dir() {
        return Vec::new();
    }
    let Ok(store) = zest_core::ThreadStore::open(root) else { return Vec::new() };
    let mut rows: Vec<ThreadSummary> =
        store.list().unwrap_or_default().into_iter().filter(|row| row.message_count > 0).collect();
    rows.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.updated_at.cmp(&a.updated_at)));
    rows.truncate(MAX_RECENT);
    rows
}

impl Workspace {
    /// Whether this window is itself the free-chats window, whose own history already lists them.
    fn is_free_chats_window(&self) -> bool {
        free_chats_root()
            .and_then(|root| std::fs::canonicalize(root).ok())
            .is_some_and(|root| root == self.project.path)
    }

    pub(super) fn load_free_chats(&mut self, cx: &mut Context<Self>) {
        if self.project.demo || self.is_free_chats_window() {
            return;
        }
        let Some(root) = free_chats_root() else { return };
        let task = cx.background_spawn(async move { list_free_chats(&root) });
        cx.spawn(async move |this, cx| {
            let rows = task.await;
            let _ = this.update(cx, |ws, cx| {
                ws.free_chats = rows;
                cx.notify();
            });
        })
        .detach();
    }

    fn open_free_chat(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(root) = free_chats_root() else { return };
        cx.defer(move |cx| {
            let opened = std::fs::create_dir_all(&root)
                .map_err(|e| e.to_string())
                .and_then(|_| Project::open(root, false).map_err(|e| e.to_string()))
                .and_then(|project| open_project_window(project, false, cx));
            if let Ok(window) = opened {
                let _ = window.update(cx, |ws, window, cx| ws.open(&id, window, cx));
            }
        });
    }

    /// Recent in the project tree, "No project" in the compact list.
    pub(super) fn recent_section(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.free_chats.is_empty() {
            return None;
        }
        let now = chrome::now_secs();
        let label = if theme::compact_sidebar() { "No project" } else { "Recent" };
        Some(
            div()
                .flex_none()
                .mt_3()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().px_2().pb_1().child(ui::section_label(Icon::Clock, label)))
                .child(
                    div()
                        .id("recent-chats")
                        .max_h(px(200.))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .children(self.free_chats.iter().map(|row| {
                            let id = row.id.clone();
                            chrome::chat_row(
                                SharedString::from(format!("free-{}", row.id)),
                                if row.pinned { Icon::Pin } else { Icon::MessageSquare },
                                chat_title(row),
                                chrome::age(row.updated_at, now),
                                false,
                                false,
                                false,
                                cx.listener(move |ws, _, _, cx| ws.open_free_chat(id.clone(), cx)),
                            )
                        })),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::list_free_chats;

    #[::core::prelude::v1::test]
    fn recent_lists_only_chats_with_messages_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list_free_chats(dir.path()).is_empty(), "a missing store lists nothing");
        let store = zest_core::ThreadStore::open(dir.path()).unwrap();
        let empty = zest_core::Thread::new();
        store.save(&empty).unwrap();
        let mut older = zest_core::Thread::new();
        older.set_title("Older").unwrap();
        older.apply_user("u1", "first");
        store.save(&older).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let mut newer = zest_core::Thread::new();
        newer.set_title("Newer").unwrap();
        newer.apply_user("u2", "second");
        store.save(&newer).unwrap();
        let titles: Vec<_> = list_free_chats(dir.path()).into_iter().map(|r| r.title.unwrap_or_default()).collect();
        assert_eq!(titles, ["Newer", "Older"]);
    }
}
