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
verify:
  - npm run ui:test
---

# Notifications and toasts

## Behavior
- An attention event shows a toast when Zest is focused and not minimized, and an OS notification otherwise (`showAttention` in `App.tsx`).
- Attention events: "Approval needed", "Input needed" (a question), "Response ready" (only for turns of 10 s or more), "Background job finished", and delegation outcomes in the active project ("Changes ready to apply", "Review requested changes", "Delegation needs attention", "Delegation failed", "Accepted changes applied", "Delegation cancelled").
- An approval in the chat being viewed sends only an OS notification, and only when away, because the approval card is already on screen. An approval in another chat gets a toast or a notification. Each approval id notifies once per turn, and each delegation event (kind, job, update time) once.
- Notification permission is asked at most once per session, on first use; if it is refused, nothing is sent and nothing fails.
- Identical toasts stack into one card with an "N identical notifications" badge. Success and info toasts group by title; errors and warnings group by title and description, so different failures stay separate.
- Toasts sit bottom-right (bottom-centre on narrow windows).

## How it works
- `notifications.ts`: `isWindowActuallyActive` asks the Tauri window (`isFocused`, `isMinimized`) and falls back to `document.hasFocus()` in a browser. `notifyWhenAway` checks that, gets permission once (`isPermissionGranted`/`requestPermission` from `@tauri-apps/plugin-notification`, or the Web Notification API outside Tauri), then calls `sendNotification`.
- Rust side: `tauri_plugin_notification::init()` in `run()` (`lib.rs`) and the `notification:default` permission in `capabilities/default.json`.
- `notificationPolicy.ts`: `LONG_TURN_NOTIFICATION_MS` / `isLongTurn`, `notificationFingerprint`, `mergeToastDescription`.
- `toast.tsx` wraps the Base UI toast manager: `toast.add` computes the fingerprint and updates the existing toast (count in `data.groupCount`) instead of adding another. `<Toaster>` wraps the whole app in `App.tsx`.
- `App.tsx` `handleChatEvent` records turn start times on the `user` event and fires on `approval_needed`, `question_needed`, `done`, and `job_completed`; `handleDelegationEvent` handles delegation outcomes.
- `backgroundFailure.ts`: `ignoreExpectedFailure(error, operation)` and `fallbackOnFailure(error, fallback, operation)` name a deliberate "do not show the user" policy for best-effort calls, logging only in dev builds.

## Verify
- `npm run ui:test` runs `notificationPolicy.test.ts` (long-turn threshold, fingerprints, description merge).
- Fixture: `/?fixture=1&scenario=approval`, send a message and look for the approval card. OS notifications need the desktop app with the window in the background.

## Pitfalls
- The lint rule `zest/no-unowned-background-rejection` rejects `.catch(() => {})` and constant handlers. Use `ignoreExpectedFailure` or `fallbackOnFailure`, and only for failures that should not change what the user sees.
- `notifications.ts` imports through the `@/` alias and the Tauri plugin, so it has no node unit test; the policy logic that needs tests lives in `notificationPolicy.ts`.
- Toasts whose title or description is not plain text are not grouped.
