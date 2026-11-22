---
title: Split workspace
summary: Open several chats side by side in resizable, nested panes, each with its own draft, model, and running turn.
paths:
  - crates/desktop/src/workspace/panes.rs
  - crates/desktop/src/workspace.rs
  - crates/desktop/src/workspace/chrome.rs
tests:
  - crates/desktop/src/workspace_test.rs
verify:
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- workspace::panes::
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- workspace::
  - npm run gpui:test
---

# Split workspace

## Behavior
- **Split** in the chat header (`Ctrl+\`) starts a split group: the current chat on the left and a new chat on the right. Each new pane is a new chat in the same project, with its own recorded start commit. There is no empty pane or chat chooser.
- In split view the header adds **Split right** and **Split down**, which split the active pane and put a new chat in the new half, plus **Zoom pane**, **Minimize pane** and **Restore all panes**. The **Split** button becomes **Single view**.
- Each pane has a 32 px header: a button with the chat title (the active pane shows a dot before it) and **Close pane**. Clicking the title, or focusing the pane's message box, makes it the active pane. Pane regions are named by their chat title.
- Opening a chat from the sidebar, search or an alert while in split view puts it in the active pane, unless it is already in the group, in which case that pane is revealed.
- Each pane is a full chat view with its own draft, attachments, queue, model and effort, and send/stop. Two panes can stream at once, and stopping one leaves the other running. Approvals and questions show inline in the pane that asked.
- The divider ("Resize split panes", announcing its percentage) stays between 30% and 70%: drag it, arrow keys move 2%, Home/End jump to the limits, and double-click resets to 50%. A divider is hidden while either side is put away.
- **Zoom** (Ctrl+Shift+Enter, a toggle) makes the active pane fill the split area. Every other pane is drawn as a strip and its view stays alive, so its turn keeps streaming and its draft survives. Toggling again restores the layout; split ratios are never changed by zoom.
- **Minimize** (Ctrl+Shift+H) puts the active pane away as a strip while its turn keeps running, and moves to the next visible pane. Beside an expanded pane a strip is a 94 px column; above or below one it is a 34 px row. Several panes can be minimized.
- A strip is a button named "Restore <title>, <status>", where status is Needs input (a pending approval or question), Running, or Idle. Clicking it opens that chat, which ends zoom and takes the pane out of the minimized set.
- A pane whose chat has a pending approval or question cannot be minimized, and the last visible pane cannot be minimized; both requests do nothing.
- Closing a pane removes it from the tree and from zoom and the minimized set. If the active pane was closed, a visible pane becomes active. Closing the second-to-last pane returns to single view.
- **Restore all panes** clears zoom and every minimized pane.
- **Single view** keeps the group in the sidebar as "Split view with N chats" and shows the active chat alone. Clicking that row reopens the group; a group that was open at the time is saved in its place. Saved groups, zoom and minimized panes last for the window's session and are not saved across a restart.
- A pane counts as on screen for alerts only while it is visible: a minimized or zoomed-away chat that needs input or finishes shows an "Open · …" banner.
- The desktop app does not have several features the old split view had: forking into another pane, an empty pane with a chat chooser, dragging a pane to another edge, a stacked layout for narrow windows, a "Continue in single view" action for one pane, a split-view command palette, and rebindable pane shortcuts.

## How it works
1. `Workspace` (`crates/desktop/src/workspace.rs`) keeps `layout: Option<Layout>` for the open group and `saved_layouts` for the others. `chats` maps each thread id to its own `Root` view, so every pane has its own command channel and turn. `split` saves the open group or calls `add_pane(Direction::Across)`; `select_chat` calls `Layout::open` so navigation fills the active pane.
2. `panes.rs` holds the model: a binary `Pane` tree (`Chat(id)` leaves and `Split { direction, fraction, first, second }` nodes), with `split`, `remove` (collapses the parent), `open`, `resize` (addressed by an `L`/`R` path, clamped to 0.30-0.70), and `ids`.
   Zoom and minimize are two fields on `Layout` (`zoom`, `minimized`) and never touch the tree. `visible` says which panes are on screen, `reveal` ends zoom and un-minimizes one pane, `restore` clears both, and `minimize` returns the pane to move to, or `None` when it would hide the last visible pane.
3. `Workspace::render_pane` draws the tree recursively. A subtree whose panes are all hidden is drawn at a fixed strip size, and the divider is drawn only between two expanded sides. The divider handles drag (`on_drag_move::<Divider>`), keys and double-click.
4. `chrome.rs` draws the header buttons and the sidebar's saved-group rows (`reopen_group`). `Ctrl+\`, Ctrl+Shift+Enter and Ctrl+Shift+H are registered in `workspace.rs` for the `Workspace` key context.
5. Each pane's composer registers `on_focus_in`, which makes that chat active when its pane is visible. The alert route in `add` treats a chat as visible when it is active, or in the layout and `visible`.

## Verify
- `cargo test ... -- workspace::panes::` checks nested splits, the ratio clamp, minimize moving to a sibling, zoom hiding the others, revealing a pane, replacing the active pane's chat, and removing a pane.
- `npm run gpui:test` (and the same scenario under `cargo test ... -- workspace::`) opens a split, adds a pane below, zooms and unzooms, minimizes, leaves and reopens the saved group, reveals a minimized pane by opening its chat, and closes a pane. After a commit it opens a fresh split and checks that its Changes report is clean while the first chat still shows the committed edit.

## Pitfalls
- Zoom and minimize must stay overrides. Never write a strip's size into `fraction`, and never drop a hidden pane from the tree: its place in the tree keeps its view, draft and live turn.
- Anything that asks "is this chat on screen?" must use `Layout::visible`, not `ids`. Counting a strip as visible stops alerts for exactly the chats the user cannot see.
- A new way to add, close or go to a pane has to decide what happens to the overrides: `split` clears zoom, `close_pane` removes the pane from both, and navigation uses `reveal`.
- `resize` addresses a split node by its path from the root. Keep the `L`/`R` path built in `render_pane` in step with the tree, or a drag can resize the wrong divider.
