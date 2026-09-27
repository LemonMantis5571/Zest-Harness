---
title: Split workspace
summary: Open several chats side by side in resizable, nested panes, each with its own draft, model, and running turn.
paths:
  - crates/desktop/ui/src/components/SplitWorkspace.tsx
  - crates/desktop/ui/src/lib/splitLayout.ts
  - crates/desktop/ui/src/lib/serialActions.ts
  - crates/desktop/ui/src/App.tsx
tests:
  - crates/desktop/ui/src/lib/splitLayout.test.ts
  - crates/desktop/ui/src/lib/serialActions.test.ts
  - crates/desktop/ui/e2e/split-workspace.spec.ts
verify:
  - npm run ui:test
  - npm run ui:e2e -- split-workspace
---

# Split workspace

## Behavior
- "Open split view" in the chat header starts a new split group: the current chat on the left ("Left chat" region), an empty chooser on the right ("Right chat"). It does nothing while the context is being compacted.
- An empty pane offers "Find a chat", a list of project chats (chats already open in the group are hidden), and "New chat in this project".
- Pane header actions: "Fork into other pane" (forks the chat into an empty or new pane; disabled while that chat is streaming), "Add pane" (fills an empty pane first, otherwise splits the pane top/bottom), "Continue in single view", "Close …", and a drag grip that moves the pane to the nearest edge of another pane.
- Nested panes are named "Left top chat", "Right bottom chat", and so on.
- Each pane keeps its own draft (saved under `zest.splitDraft.<threadId>`), model and effort, and send/stop. Two panes can stream at once, and stopping one leaves the other running.
- The divider ("Resize split panes") stays between 30% and 70%: arrow keys move 2%, Home/End jump to the limits, double-click resets to 50%. At 700 px wide or less, panes stack vertically and the divider is hidden.
- Groups survive leaving split view: the sidebar lists "Split view with N chats" and reopens the group. Panes without a chat are not counted.
- Deleting a chat empties any pane showing it and removes it from the chooser. Closing the last pane that holds a chat returns to single view, or to another saved group if there is one.
- "Continue in single view" opens that pane's chat alone with its draft carried over. Split panes show only the recent history window, with a note to continue in single view for earlier messages.

## How it works
1. `App.tsx` swaps `ChatScreen` for the lazily loaded `SplitWorkspace` when `splitVisible` is set and keeps the `SplitWorkspaceSnapshot` (`splitSnapshot`) across visits; `splitSidebarGroups` feeds `ChatHistorySidebar`.
2. `splitLayout.ts` holds the model: a binary `LayoutNode` tree (`pane` leaves and `split` nodes with `axis` row/column and `ratio`), plus the pure edits `addPaneToLayout`, `insertPaneAroundLayout`, `removePaneFromLayout` (collapses the parent), `movePaneInLayout`, `updateSplitRatio`, `clampSplitRatio`, and `createSplitWorkspaceSnapshot`/`appendSplitGroup`.
3. `SplitWorkspace.tsx` renders the tree recursively (`renderNode`, `SplitDivider`, `SplitPaneView`) and serializes its own actions with a `busy` flag (`run`).
4. The Rust side has one active session. Every pane action goes through App callbacks (`onOpen`, `onSend`, `onUpdateOptions`, `onClose`) wrapped in `createSerialActions()` (`serialActions.ts`), which re-activate that pane's chat with `openProjectChat` before acting, so activation and send cannot interleave.
5. `onSend` holds the queue until the backend acknowledges the turn (`splitSendAcks`), then releases it while the turn keeps streaming. Stop calls `cancelTurn(threadId)` for that pane only.
6. A never-sent pane chat has no durable id (`target.newThread`); sending or continuing creates the thread with `openProjectChat({ newThread: true })` instead of reopening a missing thread.

## Verify
- `npm run ui:test`: tree edits (`splitLayout.test.ts`) and the serial queue after a failure (`serialActions.test.ts`).
- `npm run ui:e2e -- split-workspace` covers concurrent streaming (`?scenario=split-streaming`), independent drafts, keyboard resize, the narrow layout, nested panes, saved groups, pointer drag, per-pane model/effort (`?scenario=model-catalogue`), and deleted chats.

## Pitfalls
- Do not call backend chat commands directly from a pane. Go through the serialized App callbacks, or a send can land in whichever chat was activated last.
- "Could not open project chat" after sending from a fresh pane means a `newThread` target was reopened by id; the e2e spec covers this path.
- Ratios are clamped in `clampSplitRatio` and again at render; keep `aria-valuemin`/`aria-valuemax` (30/70) in step if the limits change.
