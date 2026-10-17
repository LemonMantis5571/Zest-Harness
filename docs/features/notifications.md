---
title: Notifications and toasts
summary: In-app toasts when Zest is focused, OS notifications when it is not, grouped duplicates, and an explicit policy for background failures.
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
- Attention events: "Approval needed", "Input needed" (a question), "Response ready" (only for turns of 10 s or more), "Request failed", "Background job finished", and delegation outcomes in the active project ("Changes ready to apply", "Review requested changes", "Delegation needs attention", "Delegation failed", "Accepted changes applied", "Delegation cancelled").
- When Zest is not focused (or is minimized), every attention event becomes an OS notification instead of a toast.
- Approvals, questions, "Response ready", and "Request failed" are drawn in their chat's transcript (the approval card, the question, the answer, the error on the message). For the chat on screen they say nothing while Zest is focused. For another chat they show a toast.
- A chat is on screen when it is the open chat in single view with no panel (Customize, Usage, Profile, Pull requests) covering the transcript, or a pane of the split group being shown.
- Delegation outcomes live in the workbench panel, not the transcript, so they toast while focused whichever chat is open.
- The user's own clicks are not confirmed when the result is on screen: forking, rewinding, choosing a provider in the recovery dialog ("Copy opened", "Chat opened", "Provider saved"), and opening the project configuration all happen without a toast. Cancelling a delegation says nothing (its `cancelled` event is marked as the user's), and applying one shows only the "Accepted changes applied" toast from the click, not a second one from the `applied` event.
- Housekeeping is not news: automatic compaction draws a quiet divider in the transcript after the turn it followed (see [context budget](context-budget.md)) instead of a toast. Only a failed compaction warns.
- What those toasts used to explain now sits beside the control: the Rewind button's tooltip says files are not changed, and the recovery dialog says what to add to zest.toml before Configure opens it.
- Each approval id notifies once per turn, and each delegation event (kind, job, update time) once.
- Notification permission is asked at most once per session, on first use; if it is refused, nothing is sent and nothing fails.
- Identical toasts stack into one card with an "N identical notifications" badge. Success and info toasts group by title; errors and warnings group by title and description, so different failures stay separate.
- Toasts sit bottom-right (bottom-centre on narrow windows).

## How it works
- `notifications.ts`: `isWindowActuallyActive` asks the Tauri window (`isFocused`, `isMinimized`) and falls back to `document.hasFocus()` in a browser. `notifyWhenAway` checks that, gets permission once (`isPermissionGranted`/`requestPermission` from `@tauri-apps/plugin-notification`, or the Web Notification API outside Tauri), then calls `sendNotification`.
- Rust side: `tauri_plugin_notification::init()` in `run()` (`lib.rs`) and the `notification:default` permission in `capabilities/default.json`.
- `notificationPolicy.ts`: `attentionRoute(kind, { onScreen, focused, byUser })` returns `"none"`, `"toast"`, or `"os"`; `ATTENTION_KINDS` lists the kinds. Also `LONG_TURN_NOTIFICATION_MS` / `isLongTurn`, `notificationFingerprint`, `mergeToastDescription`.
- `toast.tsx` wraps the Base UI toast manager: `toast.add` computes the fingerprint and updates the existing toast (count in `data.groupCount`) instead of adding another. `<Toaster>` wraps the whole app in `App.tsx`.
- `App.tsx`: `isChatOnScreen(threadId)` reads the open thread, the shell panel, and the active split group. `showAttention(kind, threadId, notice)` asks `isWindowActuallyActive`, calls `attentionRoute`, and then toasts or calls `notifyWhenAway`. `applyChatEventNow` records turn start times on the `user` event and calls it on `approval_needed`, `question_needed`, `done`, `job_completed`, and a reducer `errorToast`; `handleDelegationEvent` calls it for delegation outcomes, with the job's `parentThreadId` as the chat. `onCancelDelegation` and `onApplyDelegation` add `cancelled:<job>` / `applied:<job>` to `ownDelegationActionsRef` before calling the backend (the event can arrive before the reply), and the handler consumes the key as `byUser`.
- `backgroundFailure.ts`: `ignoreExpectedFailure(error, operation)` and `fallbackOnFailure(error, fallback, operation)` name a deliberate "do not show the user" policy for best-effort calls, logging only in dev builds.

## Verify
- `npm run ui:test` runs `notificationPolicy.test.ts`: a table of every attention kind × on screen × focused, plus the long-turn threshold, fingerprints, and description merge.
- `npm run ui:e2e -- notifications` runs `e2e/notifications.spec.ts`: in `scenario=ui-stress` a sent turn ends with the compaction divider and no toast.
- Fixture: `/?fixture=1&scenario=approval`, send a message and look for the approval card. OS notifications need the desktop app with the window in the background.

## Pitfalls
- The lint rule `zest/no-unowned-background-rejection` rejects `.catch(() => {})` and constant handlers. Use `ignoreExpectedFailure` or `fallbackOnFailure`, and only for failures that should not change what the user sees.
- `notifications.ts` imports through the `@/` alias and the Tauri plugin, so it has no node unit test; the policy logic that needs tests lives in `notificationPolicy.ts`.
- Toasts whose title or description is not plain text are not grouped.
