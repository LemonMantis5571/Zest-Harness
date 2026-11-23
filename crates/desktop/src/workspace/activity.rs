//! Workbench > Activity and Outline for the active chat. Activity shows the session (provider,
//! model, whether a turn runs), the chat's recent tool calls, and a workspace check (`git diff
//! --check`) for whitespace problems in uncommitted changes. Outline lists your prompts and the
//! start of each reply. A row in either opens the chat at that message.

use super::*;
use crate::theme::{self, Icon};
use crate::ui::{self, Variant};
use zest_core::StoredMessage;

/// One message in the outline: its id, whether you wrote it, and its first line.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct OutlineEntry {
    pub(super) id: String,
    pub(super) user: bool,
    pub(super) line: String,
}

fn first_line(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let mut out: String = line.chars().take(100).collect();
    if line.chars().count() > 100 {
        out.push('…');
    }
    out
}

pub(super) fn outline(messages: &[StoredMessage]) -> Vec<OutlineEntry> {
    messages
        .iter()
        .filter_map(|message| match message {
            StoredMessage::User { id, text, .. } if !text.trim().is_empty() => {
                Some(OutlineEntry { id: id.clone(), user: true, line: first_line(text) })
            }
            StoredMessage::Assistant { id, text, tools, .. } => {
                let line = if text.trim().is_empty() {
                    match tools.len() {
                        0 => return None,
                        1 => "1 tool call".to_string(),
                        n => format!("{n} tool calls"),
                    }
                } else {
                    first_line(text)
                };
                Some(OutlineEntry { id: id.clone(), user: false, line })
            }
            _ => None,
        })
        .collect()
}

/// One tool call for the Activity list: the reply it belongs to, its name, status and summary.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ToolEntry {
    pub(super) message: String,
    pub(super) name: String,
    pub(super) status: String,
    pub(super) summary: String,
}

/// The chat's tool calls, newest first, at most `limit`.
pub(super) fn recent_tools(messages: &[StoredMessage], limit: usize) -> Vec<ToolEntry> {
    messages
        .iter()
        .rev()
        .filter_map(|message| match message {
            StoredMessage::Assistant { id, tools, .. } => Some((id, tools)),
            _ => None,
        })
        .flat_map(|(id, tools)| {
            tools.iter().rev().map(move |tool| ToolEntry {
                message: id.clone(),
                name: tool.name.clone(),
                status: tool.status.clone(),
                summary: first_line(tool.summary.as_deref().unwrap_or("")),
            })
        })
        .take(limit)
        .collect()
}

/// Runs `git diff --check HEAD` (staged and unstaged changes) in `root`.
pub(super) fn workspace_check(root: &std::path::Path) -> String {
    let output = std::process::Command::new("git")
        .args(["-c", "core.quotepath=off", "diff", "--check", "HEAD"])
        .current_dir(root)
        .output();
    match output {
        Err(e) => format!("Could not run git: {e}"),
        Ok(out) if out.status.success() => "No whitespace problems in uncommitted changes.".into(),
        Ok(out) if out.status.code() == Some(2) || !out.stdout.is_empty() => {
            let problems = String::from_utf8_lossy(&out.stdout);
            let lines: Vec<&str> = problems.lines().filter(|l| !l.trim().is_empty()).take(40).collect();
            format!("Whitespace problems in uncommitted changes:\n{}", lines.join("\n"))
        }
        Ok(out) => {
            let error = String::from_utf8_lossy(&out.stderr);
            format!("The check did not run: {}", first_line(&error))
        }
    }
}

impl Workspace {
    /// Opens the chat page at a message.
    fn jump_to_message(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.page(Panel::Chat, window, cx);
        self.chats[&self.active].update(cx, |r, cx| {
            if r.reveal_message(&id) {
                cx.notify();
            }
        });
    }

    fn run_workspace_check(&mut self, cx: &mut Context<Self>) {
        let root = self.project.path.clone();
        self.workspace_check = Some("Checking…".into());
        let task = native::executor().spawn_blocking(move || workspace_check(&root));
        cx.spawn(async move |this, cx| {
            let text = task.await.unwrap_or_else(|e| e.to_string());
            let _ = this.update(cx, |ws, cx| {
                ws.workspace_check = Some(text);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn activity_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let root = self.chats[&self.active].read(cx);
        let state = if root.compacting {
            "Compacting"
        } else if root.turn_active {
            "Working"
        } else {
            "Ready"
        };
        let session = root.session_label.clone();
        let tools = recent_tools(&self.chat(cx).thread.lock().unwrap().messages, 30);
        let mut body = div().flex().flex_col().gap_3();
        body = body.child(
            ui::card()
                .p_3()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().min_w_0().truncate().child(if session.is_empty() { "No turn yet".to_string() } else { session }))
                .child(ui::pill(None, state)),
        );
        body = body.child(ui::section_title(format!("Tool calls ({})", tools.len())));
        if tools.is_empty() {
            body = body.child(ui::hint("This chat has not called a tool yet."));
        }
        for (index, tool) in tools.into_iter().enumerate() {
            let message = tool.message.clone();
            body = body.child(
                div()
                    .id(SharedString::from(format!("activity-tool-{index}")))
                    .role(Role::Button)
                    .aria_label(SharedString::from(format!("{} {}", tool.name, tool.status)))
                    .cursor_pointer()
                    .px_2()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(6.))
                    .hover(|d| d.bg(rgb(theme::secondary())))
                    .child(theme::icon(crate::view::trace_icon(&tool.name), 13.))
                    .child(div().font_family(theme::MONO).text_size(px(12.)).child(tool.name))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(rgb(theme::muted_foreground())).child(tool.summary))
                    .child(div().text_size(px(11.)).text_color(rgb(theme::muted_foreground())).child(tool.status))
                    .on_click(cx.listener(move |ws, _, w, cx| ws.jump_to_message(message.clone(), w, cx))),
            );
        }
        body = body
            .child(ui::section_title("Workspace check"))
            .child(ui::hint("Looks for whitespace problems (trailing spaces, conflict markers) in uncommitted changes with git diff --check."))
            .child(ui::button("workspace-check", Variant::Outline, Some(Icon::RefreshCw), "Run check", cx.listener(|ws, _, _, cx| ws.run_workspace_check(cx))));
        if let Some(result) = &self.workspace_check {
            body = body.child(ui::card().p_3().font_family(theme::MONO).text_size(px(12.)).child(result.clone()));
        }
        body.into_any_element()
    }

    pub(super) fn outline_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let entries = outline(&self.chat(cx).thread.lock().unwrap().messages);
        let mut body = div().flex().flex_col().gap_1();
        if entries.is_empty() {
            return body.child(ui::hint("This chat has no messages yet.")).into_any_element();
        }
        for (index, entry) in entries.into_iter().enumerate() {
            let id = entry.id.clone();
            body = body.child(
                div()
                    .id(SharedString::from(format!("outline-{index}")))
                    .role(Role::Button)
                    .aria_label(SharedString::from(format!("{}: {}", if entry.user { "You" } else { "Zest" }, entry.line)))
                    .cursor_pointer()
                    .px_2()
                    .py_1()
                    .flex()
                    .gap_2()
                    .rounded(px(6.))
                    .hover(|d| d.bg(rgb(theme::secondary())))
                    .when(!entry.user, |d| d.pl_6())
                    .child(div().w(px(36.)).flex_none().text_size(px(11.)).text_color(rgb(theme::muted_foreground())).child(if entry.user { "You" } else { "Zest" }))
                    .child(div().flex_1().min_w_0().truncate().when(entry.user, |d| d.font_weight(FontWeight::MEDIUM)).child(entry.line))
                    .on_click(cx.listener(move |ws, _, w, cx| ws.jump_to_message(id.clone(), w, cx))),
            );
        }
        body.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{outline, recent_tools, workspace_check};

    fn messages() -> Vec<zest_core::StoredMessage> {
        serde_json::from_value(serde_json::json!([
            { "role": "user", "id": "u1", "text": "Fix the login\nredirect please" },
            { "role": "assistant", "id": "a1", "text": "",
              "tools": [{ "id": "t1", "name": "read_file", "status": "done", "summary": "src/login.rs" }] },
            { "role": "assistant", "id": "a2", "text": "Done. The redirect now keeps the path.",
              "tools": [{ "id": "t2", "name": "edit_file", "status": "done", "summary": "src/login.rs\n+3 -1" }] },
        ]))
        .unwrap()
    }

    #[::core::prelude::v1::test]
    fn the_outline_has_first_lines_and_counts_tool_only_replies() {
        let entries = outline(&messages());
        let lines: Vec<_> = entries.iter().map(|e| (e.id.as_str(), e.user, e.line.as_str())).collect();
        assert_eq!(
            lines,
            [("u1", true, "Fix the login"), ("a1", false, "1 tool call"), ("a2", false, "Done. The redirect now keeps the path.")]
        );
    }

    #[::core::prelude::v1::test]
    fn tool_calls_are_newest_first_and_limited() {
        let tools = recent_tools(&messages(), 10);
        let names: Vec<_> = tools.iter().map(|t| (t.name.as_str(), t.message.as_str(), t.summary.as_str())).collect();
        assert_eq!(names, [("edit_file", "a2", "src/login.rs"), ("read_file", "a1", "src/login.rs")]);
        assert_eq!(recent_tools(&messages(), 1).len(), 1);
    }

    #[::core::prelude::v1::test]
    fn the_workspace_check_reports_whitespace_errors() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git").args(args).current_dir(dir.path()).output().unwrap()
        };
        git(&["init", "-q"]);
        git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "init"]);
        std::fs::write(dir.path().join("a.txt"), "clean\n").unwrap();
        git(&["add", "a.txt"]);
        assert!(workspace_check(dir.path()).starts_with("No whitespace problems"), "{}", workspace_check(dir.path()));
        std::fs::write(dir.path().join("a.txt"), "trailing   \n").unwrap();
        let report = workspace_check(dir.path());
        assert!(report.starts_with("Whitespace problems"), "{report}");
        assert!(report.contains("a.txt"), "{report}");
    }
}
