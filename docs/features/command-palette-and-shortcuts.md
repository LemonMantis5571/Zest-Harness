---
title: Command palette and keyboard shortcuts
summary: Ctrl+K opens a command palette over actions, pages and chats; the Search page finds saved message text; fixed workspace and composer shortcuts; Escape to leave a panel or stop a turn.
paths:
  - crates/desktop/src/workspace.rs
  - crates/desktop/src/composer.rs
  - crates/desktop/src/workspace/chrome.rs
  - crates/desktop/src/workspace/palette.rs
tests:
  - crates/desktop/src/workspace_test.rs
verify:
  - npm run gpui:test
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- workspace_test
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- workspace::palette::
---

# Command palette and keyboard shortcuts

## Behavior
- Workspace shortcuts (context `Workspace`): Ctrl+K command palette, Ctrl+N new chat, Ctrl+O open a project folder, Ctrl+\ split the workspace (or return to single view), Ctrl+Shift+Enter zoom the focused pane, Ctrl+Shift+H minimize it, Escape back to the chat from a panel.
- Composer shortcuts (context `Composer`): Enter send, Shift+Enter newline, Escape stop the running turn (in the `/btw` input it cancels the side question), Ctrl+A / Ctrl+C / Ctrl+X / Ctrl+V select all, copy, cut and paste, Ctrl+. the system emoji and symbol picker, plus arrow, Shift+arrow, Home, End, Backspace and Delete editing. Ctrl+C also copies a selection in the transcript (context `Chat`).
- Shortcuts are fixed. The desktop app has no shortcuts editor and stores no overrides, and it uses Ctrl on every OS.
- Ctrl+K opens the command palette over the window (Ctrl+K again, Escape or a click outside closes it). Its field lists, in groups: **Actions** (New chat, Open folder, Split view, Zoom pane, Minimize pane, Go back, Go forward, Fork this chat, Rename this chat, Pin or unpin this chat, each with its shortcut when it has one), **Pages** (Customize, Workbench and the other pages), **Chats** (at most eight titles), and, once something is typed, **Search messages for “…”**, which opens the Search page with that text.
- Matching is case-insensitive: a label that starts with the text ranks first, then a word that starts with it, then the text anywhere, then its letters in order. An action's description matches only on the text itself.
- Up and Down move the selection (wrapping), Enter runs it, and a click runs any row.
- The sidebar's Search button opens the Search page ("Find chats, saved text and panels.").
- The Search page has one text field. It lists, in order (everything when the field is empty): Customize and Workbench pages whose name contains the text, chats whose title contains it, then (after 180 ms) saved message hits from the [shared transcript index](threads-and-history.md) as `title · excerpt`. Clicking a hit opens the chat at the matching message.
- Enter in the Search field opens the first chat whose title matches, else the first message hit for the current text, else the first matching page.
- Slash commands are not listed in the palette or on the Search page; they are in Customize > Skills & MCP (see [skills-and-slash-commands](skills-and-slash-commands.md)).
- Escape is bound in both contexts, and the deeper one wins: with focus in a composer or text field Escape goes to that field, so from a page's text field use the "Back to chat" button (shown with an Esc hint) instead.
- The desktop app has no right-click context menu.

## How it works
- Actions: `actions!(native_workspace, [NewChat, FindChat, OpenFolder, ToggleSplit, ZoomPane, MinimizePane, ClosePanel])` in `workspace.rs`; `launch_if_requested` binds them with `cx.bind_keys` after `composer::bind_keys`, and `Workspace::render` attaches one `on_action` listener per action.
- Composer: `composer.rs` declares its actions (`Send`, `Newline`, `Stop`, `ShowCharacterPalette`, the editing actions) and `bind_keys` binds them to the `Composer` context. `Stop` emits `ComposerEvent::Stop`; the chat view stops the turn and the side panel cancels its question.
- Palette: `workspace/palette.rs`. `toggle_palette` creates a `Composer` with `list_nav` set, so Up and Down emit `ComposerEvent::Navigate` instead of moving the caret; Enter (`Send`) runs the selection and Escape (`Stop`) closes it. `items` builds the ranked list from `ACTIONS`, `PAGES` and the chat summaries with `score`; `run_palette` dispatches to the same workspace methods the buttons and shortcuts use. `palette_overlay` draws the backdrop and card (dialog and list box roles for screen readers).
- Search: `Workspace::page(Panel::Search)` reloads the chat summaries; the filter field is a `Composer`. `Workspace::search` debounces 180 ms, cancels the previous query with a `CancelToken`, and runs `search::search` on the background executor. `open_hit` opens the chat and calls `reveal_message`. The Enter handling is the `ComposerEvent::Send` subscription in `Workspace::new`.
- Page names come from `PAGES` in `workspace.rs`; the page frame, tabs and "Back to chat" button come from `page_frame` in `workspace/chrome.rs`.

## Verify
- `npm run gpui:test` (and its `cargo test` run in `workspace_test.rs`) sends turns with Enter in the composer, calls the split, zoom and minimize actions directly, and renders the Search page. `workspace::palette::` tests cover ranking, grouping, the chat limit and the empty query.
- By hand: `npm run gpui -- --demo`, send a message, open the sidebar's Search, type a word from it, and click the message hit. Press Ctrl+K, type `split` and Enter to split the workspace.

## Pitfalls
- A new workspace shortcut needs an action in `actions!`, a `KeyBinding` in `launch_if_requested`, and an `on_action` listener; a binding without a listener does nothing.
- A key bound in `Composer` shadows the same key in `Workspace` while a composer has focus. Escape in the main composer stops the turn rather than closing a panel.
- Ctrl+. is the emoji picker, not "stop turn".
