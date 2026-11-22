---
title: Notifications and alerts
summary: Zest pings only when something needs the user and they are not looking at it, as an Open banner when focused or an OS notification when away, naming the chat and offering to open it.
paths:
  - crates/desktop/src/workspace.rs
  - crates/desktop/src/main.rs
  - crates/desktop/src/native_view.rs
tests:
  - crates/desktop/src/workspace_test.rs
verify:
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- attention_route_follows_window_and_chat_visibility
---

# Notifications and alerts

## Behavior
- The model: live state lives on screen, and Zest only pings when something needs the user and they are not looking. Every attention decision uses two facts: whether the Zest window is active and whether the event's chat is on screen.
- Attention events: "Approval needed", "Input needed" (a question), "Request failed", and "Response ready" (only for turns of 10 s or more that were not stopped, cancelled or a manual compaction).
- When the Zest window is not active, every attention event becomes an OS notification. Its title is the event and its body the chat's title (else "New chat"). Selecting it opens that chat in its window and brings the window forward.
- When the window is active and the chat is on screen, nothing is added: the approval or question card, the answer, or the error is already in front of the user.
- When the window is active and the chat is not on screen, a banner button appears at the top of the workspace: "Open · Approval needed · Fix the login redirect". Clicking it opens the chat. Each chat and event kind has at most one banner.
- A chat is on screen when the chat page is showing (no Customize, Workbench or other panel covering it) and the chat is the active one or a visible pane of the split group. A minimized pane, or one hidden behind a zoomed pane, is not on screen.
- Approval and question alerts are raised only while their card is still pending. When the chat next changes (it sends a message or its turn ends), its approval and question banners whose request was answered or dropped go away. Opening a chat removes all its banners; "Request failed" and "Response ready" banners stay until then.
- Errors and other messages for the chat on screen go to the notice card above the composer, which stays until dismissed. Workspace-level problems (for example a chat that cannot be loaded, or a cancelled sign-in) show in a notice bar at the top of the workspace.
- The desktop app does not show toasts, and does not notify about background jobs or delegation outcomes; those are visible in Workbench > Jobs and the Delegation board. It does not ask for notification permission itself; the OS decides whether a notification is shown.

## How it works
- `main.rs` `Root::apply` emits `workspace::ChatAttention` events: `Approval` or `Question` when an `EvKind::Card` arrives, `Failed` on `EvKind::Failure`, and `Ready` on `End` when the turn ran at least 10 s and was not stopped, cancelled or compacting.
- `workspace.rs` subscribes to each chat view it adds. The handler first checks `ChatAttention::pending` (a matching card is still queued; always true for `Failed` and `Ready`), works out visibility from the current panel, the active chat and the split layout, and asks `chat_alert_route(window_active, visible)` for `Quiet`, `Banner` or `System`.
- `System` calls `cx.show_system_notification` with the tag `<workspace entity id>/<chat id>`. The handler registered with `cx.on_system_notification_response` at startup splits the tag, finds the matching workspace window, opens the chat, and activates the window.
- `Banner` pushes `(chat id, kind, label)` onto `Workspace::alerts` unless the same chat and kind is already there; `Workspace::render` draws them as "Open · …" buttons. The `ChatChanged` subscription drops alerts whose request is no longer pending, and `select_chat` drops every alert for the chat being opened.
- The notice card is `Root::note`, drawn by `native_view.rs` `native_controls` with a dismiss button; the workspace notice bar is `Workspace::note`.

## Verify
- `attention_route_follows_window_and_chat_visibility` (in `workspace_test.rs`) checks the route table: active and visible is quiet, active and hidden is a banner, inactive is a system notification either way.
- By hand with a configured provider (the offline demo answers too fast to look away): ask for a change that needs approval, then open Customize before the request arrives; an "Open · Approval needed · …" banner appears below the header and opens the chat. Repeat with focus moved to another application; an OS notification appears, and selecting it opens the chat.

## Pitfalls
- The pending check runs when the alert is raised and again on every `ChatChanged`; answering a card does not emit `ChatChanged`, so an answered request's banner can remain until the chat's turn ends or the chat is opened.
- The notification tag includes the workspace entity id so a click reaches the right window when several projects are open.
- Approval and question cards are dropped when the turn ends; the same `End` handling emits `ChatChanged`, which clears their banners.
