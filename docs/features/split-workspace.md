---
title: Split workspace
summary: Open several chats side by side in resizable, nested panes, each with its own draft, model, and running turn.
paths:
  - crates/desktop/ui/src/components/SplitWorkspace.tsx
  - crates/desktop/ui/src/components/SplitPaneStrip.tsx
  - crates/desktop/ui/src/lib/splitLayout.ts
  - crates/desktop/ui/src/lib/splitPaneStatus.ts
  - crates/desktop/ui/src/lib/serialActions.ts
  - crates/desktop/ui/src/App.tsx
tests:
  - crates/desktop/ui/src/lib/splitLayout.test.ts
  - crates/desktop/ui/src/lib/splitPaneStatus.test.ts
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
- Pane header actions: "Fork into other pane" (forks the chat into an empty or new pane; disabled while that chat is streaming), "Add pane" (fills an empty pane first, otherwise splits the pane top/bottom), "Zoom pane", "Minimize pane", "Continue in single view", "Close …", and a drag grip that moves the pane to the nearest edge of another pane.
- Nested panes are named "Left top chat", "Right bottom chat", and so on.
- Each pane keeps its own draft (saved under `zest.splitDraft.<threadId>`), model and effort, and send/stop. Two panes can stream at once, and stopping one leaves the other running.
- A pane draws a turn's thinking and tool calls as the same Trace as single view (`AssistantTrace`). A pane has no approval bar above its composer, so a pending approval's card shows inline under the answer.
- The divider ("Resize split panes") stays between 30% and 70%: arrow keys move 2%, Home/End jump to the limits, double-click resets to 50%. At 700 px wide or less, panes stack vertically and the divider is hidden.
- Groups survive leaving split view: the sidebar lists "Split view with N chats" and reopens the group. Panes without a chat are not counted.
- **Zoom** ("Zoom pane", a toggle with `aria-pressed`, also a command and a shortcut) makes one pane fill the split area. Every other pane is drawn as a strip and stays mounted, so its turn keeps streaming and its draft survives. Toggling again restores the layout exactly: zoom is a render-time override, and no split ratio is ever edited. It needs a second pane. Zoom never undoes itself when a hidden pane needs input; the strip shows the attention state.
- **Minimize** ("Minimize pane", `aria-expanded`, also a command and a shortcut) puts a pane away as a strip while its turn keeps running. The strip is a button named "Restore <title>, <status>" that restores the pane and moves keyboard focus into its message box. Beside an expanded pane a strip is a thin column; above or below one, and at 700 px wide or less where panes stack, it is a short row. A group can have several minimized panes, and zoom and minimize combine: ending zoom brings back the earlier minimized set as it was.
- A strip shows the chat title and live status, in this order: awaiting approval or waiting for your answer (`needsInput`), working, unread, otherwise idle. A pane with no chat can be a strip only behind zoom, and reads "Empty".
- A minimized pane whose chat needs input restores itself without taking focus, and cannot be minimized again while the request is pending ("Answer the pending request before minimizing this pane"). The last visible pane cannot be minimized ("Keep at least one pane visible"), and a pane with no chat cannot be minimized.
- Rules for edits while panes are put away: adding a pane, forking into a hidden empty pane, or "Add pane" restores everything first, so a new pane never starts hidden. Closing a pane drops it from zoom and the minimized set; if that leaves no visible pane, the strips come back. Deleting a chat un-minimizes its pane. A pane cannot be dragged onto a strip, and a pane going away moves the focused pane to a visible one. Going to a pane from the sidebar, an alert's Open action, or a strip reveals it: zoom ends and the pane leaves the minimized set; other strips stay.
- "Restore all panes" clears zoom and every minimized pane. The zoom and minimized state lives in the in-memory `SplitWorkspaceSnapshot`, so it survives leaving and reopening split view, and is not saved across an app restart.
- Split view has its own Mod+K palette, limited to pane actions: "Zoom pane", "Minimize pane" and "Restore all panes" (no chats, slash commands or filter tabs, because no single chat owns the view). The pane shortcuts and the palette act on the pane that holds keyboard focus, else the group's focused pane. Defaults are Mod+Shift+Enter (zoom) and Mod+Shift+H (minimize); restore has no default chord. All three are rebindable under Customize > Shortcuts > Split view (see [command palette and shortcuts](command-palette-and-shortcuts.md)).
- All panes showing their transcripts count as read, regardless of which chat
  the backend last activated. A chat covered by its pane's chooser, minimized,
  or hidden behind a zoomed pane stays off screen: completion marks it unread,
  and pending requests can notify. Cancel in the chooser reveals the
  transcript and clears its unread mark; restoring a strip does the same. An
  alert's Open action also closes the chooser and reveals its chat, ending zoom
  or a minimize. Reopening a saved group clears unread marks on its visible
  transcripts.
- Deleting a chat empties any pane showing it and removes it from the chooser. Closing the last pane that holds a chat returns to single view, or to another saved group if there is one.
- "Continue in single view" opens that pane's chat alone with its draft carried over. Split panes show only the recent history window, with a note to continue in single view for earlier messages.

## How it works
1. `App.tsx` swaps `ChatScreen` for the lazily loaded `SplitWorkspace` when `splitVisible` is set and keeps the `SplitWorkspaceSnapshot` (`splitSnapshot`) across visits; `splitSidebarGroups` feeds `ChatHistorySidebar`.
2. `splitLayout.ts` holds the model: a binary `LayoutNode` tree (`pane` leaves and `split` nodes with `axis` row/column and `ratio`), plus the pure edits `addPaneToLayout`, `insertPaneAroundLayout`, `removePaneFromLayout` (collapses the parent), `movePaneInLayout`, `updateSplitRatio`, `clampSplitRatio`, and `createSplitWorkspaceSnapshot`/`appendSplitGroup`.
   Zoom and minimize are two fields on `SplitGroup` (`zoomedPaneId`, `minimizedPaneIds`) and never touch the tree. `resolvePaneOverrides` makes them consistent with the tree and chats (closed panes dropped, empty panes never minimized, never every pane folded), `collapsedPaneIds` says which panes are strips (all but the zoomed one, else the minimized ones), and `visiblePaneIds` which panes' transcripts are on screen. `toggleZoomPane`, `toggleMinimizePane`, `revealPane`, `restoreAllPanes`, `wakeMinimizedPanes`, and `pruneGroupOverrides` are the edits; `isNodeCollapsed` and `stripOrientation` drive drawing.
3. `SplitWorkspace.tsx` renders the tree recursively (`renderNode`, `SplitDivider`, `SplitPaneView`) and serializes its own actions with a `busy` flag (`run`). A strip is drawn by `SplitPaneStrip.tsx` inside the same `SplitPaneView`; the pane's header and body are only hidden (`display: none`), never unmounted, and a subtree with no expanded pane is a run of strips whatever its axis was. Divider ratios apply only while both sides are expanded. Status comes from `splitPaneStatus.ts`, fed by `threadActivity` (passed from `App.tsx`) and the unread set. An effect restores minimized panes that need input. `useKeybindings` registers the pane shortcuts and the actions-only `CommandPalette` (`scope="actions"`).
4. The Rust side has one active session. Every pane action goes through App callbacks (`onOpen`, `onSend`, `onUpdateOptions`, `onClose`) wrapped in `createSerialActions()` (`serialActions.ts`), which re-activate that pane's chat with `openProjectChat` before acting, so activation and send cannot interleave.
5. `onSend` holds the queue until the backend acknowledges the turn (`splitSendAcks`), then releases it while the turn keeps streaming. Stop calls `cancelTurn(threadId)` for that pane only.
6. A never-sent pane chat has no durable id (`target.newThread`); sending or continuing creates the thread with `openProjectChat({ newThread: true })` instead of reopening a missing thread.
7. `onStateChange` reports the workspace and the chooser's pane id to App so
   unread and attention decisions use transcript visibility; App's
   `visibleChatThreads` reads `visiblePaneIds`, so strips are off screen.
   `activateSplitLocation` reveals the pane it goes to. A new snapshot
   from App applies external pane/group focus and dismisses the chooser; a
   snapshot echoed from the component's own report keeps its local chooser.
   Approval decisions and question answers go through App's shared per-chat
   handlers, so optimistic resolution and retryable rollback update the same
   state used by pending-request guards. Applying external navigation does not
   echo the old snapshot back to App.

## Verify
- `npm run ui:test`: tree edits and the zoom/minimize overrides (`splitLayout.test.ts`: restore keeps the tree and ratios, the last visible pane stays, closing and chat deletion prune, wake leaves zoom alone), strip status (`splitPaneStatus.test.ts`), and the serial queue after a failure (`serialActions.test.ts`).
- `npm run ui:e2e -- split-workspace` covers concurrent streaming (`?scenario=split-streaming`), independent drafts, keyboard resize, the narrow layout, nested panes, saved groups, pointer drag, per-pane model/effort (`?scenario=model-catalogue`), and deleted chats.
- The zoom and minimize specs use two panes that each stream a turn. They check that a pane behind a strip keeps streaming and keeps its draft, that restoring returns the same widths and divider ratio, that a minimized or zoomed-away pane finishing reads unread (and a visible one does not) and that restoring clears it, that a minimized pane restores itself on an approval or a question without taking focus, that an approval behind zoom keeps zoom and toasts with an Open that ends it, the 700 px layout, adding or closing a pane, leaving and reopening split view, the shortcuts, the palette, and a rebound chord.
- Fixture: `?scenario=split-streaming` keeps every sent turn live until it is stopped. The browser event `zest:fixture-split-turn` (`detail: { prompt, action, text? }`, `action` one of `delta`, `done`, `approval`, `question`; `prompt` is the text that started the turn) moves a turn along without timers. A turn with no event never finishes by itself, so a pane stays "Working". Look at it by hand with `node scripts/zest-control.mjs start --scenario split-streaming`, then "Open split view" and "Fork into other pane".
- The chooser regression opens the working chat from `sidebar-status`, covers
  it with Find a chat, then dispatches `zest:fixture-finish-working`. It checks
  that the chooser and its query survive parent echoes and turn completion,
  and the chat stays unread until Cancel restores the transcript. The
  notification spec separately covers a chooser-hidden approval and Open.

## Pitfalls
- Do not call backend chat commands directly from a pane. Go through the serialized App callbacks, or a send can land in whichever chat was activated last.
- "Could not open project chat" after sending from a fresh pane means a `newThread` target was reopened by id; the e2e spec covers this path.
- Ratios are clamped in `clampSplitRatio` and again at render; keep `aria-valuemin`/`aria-valuemax` (30/70) in step if the limits change.
- Zoom and minimize must stay overrides. Never write a strip's size into `ratio`, and never filter a collapsed pane out of the tree being rendered: its position in the tree is what keeps it mounted, with its draft, scroll position, and live turn.
- Anything that asks "is this chat on screen?" must use `visiblePaneIds`, not `layoutPaneIds`. Counting a strip as visible silently stops unread marks and attention toasts for exactly the chats the user cannot see.
- A new way to add, close, or reveal a pane has to decide what happens to the overrides: `restoreAllPanes` for a new pane, `pruneGroupOverrides` after removing one, `revealPane` for navigation to one.
- Strips are `display: none` siblings of the pane body, so a pane's `textarea` and buttons are not reachable (or focusable) while it is a strip. Anything that needs to act on a stripped pane should go through its strip or reveal it first.
- The strip and the 700 px rules live in `index.css` next to `.zest-split-node`, and the narrow rule forces every wrapper to `flex: 1 1 0%` with `!important`; collapsed wrappers opt out with `.zest-split-collapsed`.
