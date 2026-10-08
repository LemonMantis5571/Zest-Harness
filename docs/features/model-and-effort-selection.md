---
title: Model and effort selection
summary: Pick a model and reasoning effort from the composer, search across providers, and have the choice remembered per provider in each project.
paths:
  - crates/desktop/ui/src/components/ModelEffortPicker.tsx
  - crates/desktop/ui/src/components/ModelIcon.tsx
  - crates/desktop/ui/src/lib/models.ts
  - crates/desktop/ui/src/lib/modelMarks.ts
  - crates/desktop/ui/src/lib/sessionOptions.ts
  - crates/desktop/ui/src/lib/useOptionNavigation.ts
  - crates/desktop/ui/src/lib/providerSelection.ts
  - crates/core/src/prefs.rs
tests:
  - crates/desktop/ui/src/lib/models.test.ts
  - crates/desktop/ui/src/lib/modelMarks.test.ts
  - crates/desktop/ui/src/lib/sessionOptions.test.ts
  - crates/desktop/ui/src/lib/providerSelection.test.ts
  - crates/desktop/ui/e2e/model-picker.spec.ts
  - crates/desktop/ui/e2e/model-picker-search.spec.ts
  - crates/desktop/ui/e2e/picker-keyboard.spec.ts
verify:
  - npm run ui:test
  - npm run ui:e2e -- model-picker
  - npm run ui:e2e -- picker-keyboard
  - cargo test -p zest-core --lib prefs::tests
  - cargo test -p zest-core --lib remembered
  - cargo test -p zest-core --lib provider::tests
---

# Model and effort selection

Intended behavior was specified in
[plans/001](../../plans/001-model-selection-reliability.md),
[002](../../plans/002-picker-keyboard-navigation.md) and
[004](../../plans/004-searchable-model-picker.md). Catalogues come from the
provider drivers in [provider-runtime](provider-runtime.md).

## Behavior

- Availability comes only from Rust (`SessionInfo.models`, `ProviderView.models`);
  `MODEL_LABELS` in `models.ts` is display text only. Unknown ids show raw.
- The composer shows the picker only when there is a real choice
  (`modelPickerHasChoices`: more than one group or more than one model);
  otherwise a static chip with the model (and effort, when supported).
- Groups: the current provider first, then every other *selectable* provider with
  models. A provider whose catalogue equals the current one (ChatGPT Codex vs
  Codex CLI) is not shown twice.
- Picking a model in another provider's group switches provider
  (`onSwitchProvider(providerId, model)`), with the same thread/copy rules as the
  provider sheet.
- Picking a model with efforts keeps the panel open and moves focus to the
  selected effort once the save finishes; a model without efforts closes the
  panel and returns focus to the trigger. Picking an effort closes the panel.
- Efforts shown are the model's list. An explicitly empty list means "No effort
  control" and hides the effort row; missing capability data falls back to
  `low`–`max`.
- While a save is pending every option and Reset are disabled, the panel shows
  "Saving selection…" and `aria-busy`. A failed save rolls back model and effort
  and toasts "Could not update model" / "Could not update effort".
- Dismissing (Escape or pointer outside) during a save stays dismissed; Escape
  restores focus to the trigger.
- Search filters case-insensitively by provider label or model id / label,
  keeps group order and identity, never changes the selection, shows "No models
  match your search." and a Clear button. Arrow keys from search enter the list.
- Keyboard: ArrowUp/Down wrap, Home/End jump in the model list; Left/Right in
  the effort row. Focus moves without selecting; Enter/Space commit.
- Reset to default sets the provider's default model and effort `high` in one
  request and clears the remembered values.
- The choice is sticky per provider per project in `.zest/session-state.json`.
  A remembered model or effort the provider no longer offers is dropped at
  session start instead of making the provider unselectable; an explicitly
  requested one that does not fit is an error.

## How it works

1. `ChatScreen` computes `modelPickerGroups` / `modelPickerHasChoices` and renders
   `Composer` → `ModelEffortPicker` (also used per pane by `SplitWorkspace`).
2. Picker helpers: `filterModelPickerGroups`, `effortsForModel`,
   `formatContextWindow` (`lib/models.ts`); roving focus from
   `useOptionNavigation`; marks from `ModelIcon` → `modelMark` (`modelMarks.ts`)
   or the provider brand (`providerMarks.ts`).
3. `App.tsx` `commitSessionOptions` applies the change optimistically, calls
   `backend.updateSessionOptions` or `resetSessionOptions`, then
   `mergeSessionOptions` / `rollbackSessionOptions` (`lib/sessionOptions.ts`).
   A second request while one is pending is ignored (`optionsUpdatingRef`).
4. Tauri `update_session_options` (`crates/desktop/src/lib.rs`) requires an idle
   session, normalizes effort, calls `agent.validate_options`, then
   `persist_provider_model_effort` → `ProjectSessionState::set_model_effort`
   (`prefs.rs`, atomic JSON write). `reset_session_options` uses the descriptor's
   default model and `high`, then `clear_model_effort`.
5. On session start, `start_session_inner` loads
   `ProjectSessionState::load(root, id)` and passes the values to
   `RuntimeBuilder::with_remembered_options` (`runtime.rs`), which drops values
   the provider cannot serve; `with_model` / `with_effort` are the strict path.
6. Provider switch from the picker: `switchProvider` → `switch_session_provider`.

## Verify

- Fixture scenarios: `/?fixture=1&scenario=options-delayed` (slow saves),
  `options-failing` (first save fails), `model-catalogue` (long catalogue,
  several providers, a no-effort model).
- `model-picker.spec.ts`: pending lock, rollback, dismiss during save, reset.
  The pending-lock check controls the fixture clock so the save cannot finish
  before every disabled control is inspected, then advances it to check persistence.
  `model-picker-search.spec.ts`: search, capabilities, long catalogue at 1280 and
  720 px. `picker-keyboard.spec.ts`: keyboard paths and focus restoration.

## Pitfalls

- Never use a portal menu here; portals have crashed the desktop webview. The
  panel is a positioned `div`.
- `prefs.rs` migrates only project-level legacy `last-model` / `last-effort` /
  `last-thread-id`, and only when the sticky thread belongs to the active
  provider. Reading user-level legacy files once leaked a Codex model into the
  Claude slot and made Claude unselectable.
- `DEFAULT_CODEX_MODEL` in `models.ts` is only a fallback label source, not an
  availability list; `CODEX_MODELS` is legacy fixture data.
- Current choices include GPT-6.1 Sol and GPT-6 Astra on both Codex transports,
  and Claude Opus 5.5, Sonnet 5.5, Haiku 5.5 and Fable 5.1 on the native API
  and as explicit Claude Code ids. Saved selections and configured defaults are
  unchanged. The builtin Codex list no longer offers retired GPT-5.4 choices;
  a configured model allow-list remains authoritative.
- `providerSupportsModelPicker` is deprecated; use `sessionSupportsModelPicker`.
