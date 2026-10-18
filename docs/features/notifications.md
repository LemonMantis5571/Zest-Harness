---
title: Notifications and toasts
summary: Zest pings only when something needs the user and they are not looking at it, as a toast when focused or an OS notification when away, naming the chat and offering to open it.
paths:
  - crates/desktop/ui/src/lib/notifications.ts
  - crates/desktop/ui/src/lib/notificationPolicy.ts
  - crates/desktop/ui/src/lib/backgroundFailure.ts
  - crates/desktop/ui/src/components/ui/toast.tsx
  - crates/desktop/ui/src/App.tsx
  - crates/desktop/capabilities/default.json
tests:
  - crates/desktop/ui/src/lib/notificationPolicy.test.ts
  - crates/desktop/ui/e2e/notifications.spec.ts
verify:
  - npm run ui:test
  - npm run ui:e2e -- notifications
---

# Notifications and toasts

## Behavior
- The model: live state lives on screen, and Zest only pings when something needs the user and they are not looking. `attentionRoute` in `notificationPolicy.ts` makes every attention decision from three facts: the event kind, whether its chat is on screen, and whether Zest is focused.
- Attention events: "Approval needed", "Input needed" (a question), "Response ready" (only for turns of 10 s or more), "Request failed", "Background job failed", "Background job finished", and delegation outcomes in the active project ("Changes ready to apply", "Review requested changes", "Delegation needs attention", "Delegation failed", "Accepted changes applied", "Delegation cancelled").
- When Zest is not focused (or is minimized), every attention event becomes an OS notification instead of a toast.
- Every attention toast and OS notification names its chat: "Approval needed · Fix the login redirect". The name is the sidebar title, else the chat's first prompt, else "Untitled chat". "Response ready" carries the start of the reply as plain text instead of a generic line.
- An attention toast for a chat that is not on screen has an "Open" button. It focuses that chat's split pane, closes the panel covering the transcript, or opens the chat.
- Clicking an OS notification opens the chat only where the platform reports the click: the Web Notification API does (fixture/browser mode). The Tauri notification plugin does not on desktop; it uses only title, body, icon and sound there, and its `onAction` listener is mobile-only. In the desktop app a click raises the window where the OS does that, and the chat is not opened.
- Approvals, questions, "Response ready", and "Request failed" are drawn in their chat's transcript (the approval card, the question, the answer, the error on the message). For the chat on screen they say nothing while Zest is focused. For another chat they show a toast.
- A chat is on screen when it is the open chat in single view with no panel (Customize, Usage, Profile, Pull requests) covering the transcript, or a pane of the split group showing its transcript. A pane showing its chat chooser does not count, even though it keeps the previous chat's session.
- A finished background job is already reported to the agent by a notice injected into its chat, so a job that completes (or is killed on request) only sends an OS notification when the user is away. A job with status `failed` toasts while focused.
- Delegation outcomes live in the workbench panel, not the transcript, so they toast while focused whichever chat is open.
- The user's own clicks are not confirmed when the result is on screen: forking, rewinding, choosing a provider in the recovery dialog ("Copy opened", "Chat opened", "Provider saved"), and opening the project configuration all happen without a toast. Cancelling a delegation says nothing (its `cancelled` event is marked as the user's), and applying one shows only the "Accepted changes applied" toast from the click, not a second one from the `applied` event.
- Housekeeping is not news: automatic compaction draws a quiet divider in the transcript after the turn it followed (see [context budget](context-budget.md)) instead of a toast. Only a failed compaction warns.
- What those toasts used to explain now sits beside the control: the Rewind button's tooltip says files are not changed, and the recovery dialog says what to add to zest.toml before Configure opens it.
- Each approval id notifies once per turn, and each delegation event (kind, job, update time) once.
- Notification permission is asked at most once per session, on first use; if it is refused, nothing is sent and nothing fails.
- Timing: an error toast stays until dismissed, a success toast lasts 3 s, and warnings and info keep Base UI's 5 s, unless the caller sets `timeout`. Attention toasts set their own: "Approval needed", "Input needed", "Request failed" and "Background job failed" stay until dismissed; the rest last 8 s so "Open" can be reached. Hovering a toast pauses its timer.
- A toast for a transcript event (approval, question, Response ready, Request failed) closes as soon as its chat is on screen, however it got there. Approval and question toasts also close when that chat's turn ends.
- Identical toasts stack into one card with an "N identical notifications" badge. Success and info toasts group by title; errors and warnings group by title and description, so different failures stay separate.
- Attention toasts also group by chat and event kind. Identical names and descriptions in two chats keep separate Open actions and dismissal ownership.
- Approval and question alerts recheck the pending request after chat lookup and notification permission waits. A resolved request or terminal turn cannot create a late alert.
- Every approval decision, including Deny, immediately clears its pending
  state in the owning chat. Answering a question clears its questionnaire
  before the tool result arrives. Single and split views share these handlers.
  Retryable failures restore the request only while the same turn and tool
  are still live. Dropped waiters and completed turns never restore it.
- Toasts sit bottom-right (bottom-centre on narrow windows).

## How it works
- `notifications.ts`: `isWindowActuallyActive` asks the Tauri window (`isFocused`, `isMinimized`) and falls back to `document.hasFocus()` in a browser. `notifyWhenAway` checks that, gets permission once (`isPermissionGranted`/`requestPermission` from `@tauri-apps/plugin-notification`, or the Web Notification API outside Tauri), then calls `sendNotification`. Its `onClick` is attached only to a Web `Notification`.
- Rust side: `tauri_plugin_notification::init()` in `run()` (`lib.rs`) and the `notification:default` permission in `capabilities/default.json`.
- `notificationPolicy.ts`: `attentionRoute(kind, { onScreen, focused, byUser })` returns `"none"`, `"toast"`, or `"os"`; `ATTENTION_KINDS` lists the kinds. `attentionChatName`, `attentionTitle`, and `replyExcerpt` write the copy. `toastTimeout(type)` and `attentionToastTimeout(kind)` set lifetimes; `shownInTranscript` and `WAITS_FOR_USER` say which toasts close when their chat opens or their turn ends. Also `LONG_TURN_NOTIFICATION_MS` / `isLongTurn`, `notificationFingerprint`, `mergeToastDescription`.
- `toast.tsx` wraps the Base UI toast manager: `toast.add` fills in `timeout` from `toastTimeout` when the caller left it out, computes the fingerprint and updates the existing toast (count in `data.groupCount`) instead of adding another. `<Toaster>` wraps the whole app in `App.tsx`.
- `App.tsx`: `isChatOnScreen(threadId)` reads the open thread, the shell panel,
  and the active split group's panes, excluding the pane whose chooser is open.
  `showAttention(kind, threadId, notice)` asks `isWindowActuallyActive` and
  calls `attentionRoute`; unless the route is `none`, `describeChat` names the
  chat from `listChatProjects` or its first prompt, and `chatOpener` finds how
  to reach it. It toasts with `attentionToastTimeout(kind)` and an Open action
  when off screen, or calls `notifyWhenAway(title, body, { onClick, isCurrent })`.
  Transcript-event toasts are kept in `attentionToastsRef` by chat. An effect
  on visible threads closes those now on screen; `done`/`error`/`cancelled`
  close that chat's approval and question toasts. `onResolveApproval` and
  `onResolveQuestion` update `chatStatesRef` for optimistic decisions and
  rollback, and update the active transcript only when it still owns that chat.
  A dropped approval is retired; a dropped question stays cleared. Both views
  use these handlers. `applyChatEventNow` handles chat
  attention events; `handleDelegationEvent` uses the job's `parentThreadId`.
  `onCancelDelegation` and `onApplyDelegation` record their own actions before
  the backend call, so an event arriving before the reply is still recognized
  as `byUser`.
- `backgroundFailure.ts`: `ignoreExpectedFailure(error, operation)` and `fallbackOnFailure(error, fallback, operation)` name a deliberate "do not show the user" policy for best-effort calls, logging only in dev builds.

## Verify
- `npm run ui:test` runs `notificationPolicy.test.ts`: a table of every attention kind × on screen × focused, plus the long-turn threshold, fingerprints, and description merge.
- `npm run ui:e2e -- notifications` runs `e2e/notifications.spec.ts` with `document.hasFocus` pinned to true (a headless page need not report focus, and zest-control's page does not, so its sessions only ever take the OS path): in `scenario=background-approval` the Free chat's approval toasts "Approval needed · Free chat" and Open shows its card, and the toast is still there after 6 s and closes when the chat is opened from the sidebar; in `scenario=approval` the on-screen approval shows only its card; in `scenario=ui-stress` a sent turn ends with the compaction divider and no toast.
- Fixture: `/?fixture=1&scenario=approval`, send a message and look for the approval card. `/?fixture=1&scenario=background-approval`, send a message in a focused browser window and look for the "Approval needed · Free chat" toast. OS notifications need the desktop app with the window in the background.
- Browser regressions also create identical alerts for two chats and open both, and hold chat lookup while cancelling a background approval to check that no stale alert appears.
- The background approval fixture also accepts Allow and keeps the command
  running until `cancelTurn`, without emitting a tool result. Adding
  `&failBackgroundApprovalOnce=1` makes its first decision fail so the browser
  tests can check rollback for Allow or Deny. `&holdInteractionResults=1` in
  `background-approval` or `sidebar-status` holds Deny's or the seeded question's
  result after the backend accepts the decision. The fixture-only browser event
  `zest:fixture-release-interaction-result` accepts a seeded thread id of
  `fixture-free`, `fixture-local`, or `fixture-question` to release that result;
  `cancelTurn` clears it. Tests defer chat lookup across Allow and Deny, and
  browser notification permission across answers in single and split view.
  They also check question retry, dropped waiters, and cancellation during a
  failure. A split chooser test checks Open reveals the transcript and that
  Allow works in that pane. No production backend controls are added.

## Pitfalls
- The lint rule `zest/no-unowned-background-rejection` rejects `.catch(() => {})` and constant handlers. Use `ignoreExpectedFailure` or `fallbackOnFailure`, and only for failures that should not change what the user sees.
- `notifications.ts` imports through the `@/` alias and the Tauri plugin, so it has no node unit test; the policy logic that needs tests lives in `notificationPolicy.ts`.
- Toasts whose title or description is not plain text are not grouped.
