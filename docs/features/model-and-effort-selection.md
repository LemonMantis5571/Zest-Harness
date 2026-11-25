---
title: Model and effort selection
summary: Pick a provider, model and reasoning effort for a chat in Customize > Models, and have the choice remembered per provider in each project.
paths:
  - crates/core/src/prefs.rs
  - crates/desktop/src/workspace.rs
  - crates/desktop/src/native.rs
  - crates/desktop/src/native_view.rs
tests:
  - crates/desktop/src/workspace_test.rs
verify:
  - cargo test -p zest-core --lib prefs::tests
  - cargo test -p zest-core --lib remembered
  - cargo test -p zest-core --lib provider::tests
  - npm run gpui:test
---

# Model and effort selection

Catalogues come from the provider drivers in
[provider-runtime](provider-runtime.md).

## Behavior

- The composer toolbar has a model chip (`choose-model`, accessible name
  "Model: <model>") that shows the model part of the chat's session line.
  Clicking it, or sending exactly `/model`, opens Customize > Models for that
  chat. The desktop app has no in-composer model panel or model search.
- Customize > Models (`Panel::Settings`) shows, in order: Provider, Model,
  Reasoning effort and Tool permissions. Each choice is a button; the current
  one is marked.
- Provider buttons list every `[providers.*]` entry in the active config. Picking
  another provider for a chat with messages switches it, or continues it in a
  copy when a vendor CLI owns either side (see
  [providers-and-sign-in](providers-and-sign-in.md)); a hint above the buttons
  says so.
- Models come from `zest_core::descriptor_from_config` for the selected
  provider, so availability is decided in Rust. The current model is the
  remembered one, else the descriptor's `default_model`. Alias resolution
  ("opus → Opus 5") and the too-old-CLI warning are not shown in the desktop
  app.
- Efforts shown are the selected model's `efforts` list. A model with an empty
  list has no Reasoning effort section.
- Choices are refused while a turn is running; the panel says "Model changes
  are available when this turn finishes."
- Picking a provider clears the model and effort (provider defaults apply).
  Picking a model clears the effort. Picking an effort keeps the model.
- After a choice the chat's session line reads `provider · model · effort`,
  with "default model" / "default effort" for unset values.
- In the offline demo the panel says "Offline demo provider"; with no
  configured provider it says "Configure a provider in zest.toml to start a
  turn."
- The choice is sticky per provider per project in `.zest/session-state.json`
  (which also stores the project's default `approvalMode` selected under Tool
  permissions). The desktop app has no Reset to default control.

## How it works

1. `Chat::new` (`native.rs`) picks the provider (the thread's `provider_id`,
   else `Project::default_provider`: the config's default target, else the
   first configured provider), then loads
   `ProjectSessionState::load(root, provider)` and falls back to the default
   target's model and effort.
2. `Workspace::select_model` (`workspace.rs`) ignores the call during a turn,
   rejects a provider other than the thread's owner, writes the provider's
   model and effort with `ProjectSessionState::save` (`prefs.rs`, atomic JSON
   write), replaces `Chat.options`, and updates the chat's `session_label`.
   A save error appears as the workspace note.
3. Each turn, `Chat::agent` builds the runtime with
   `RuntimeBuilder::with_model` / `with_effort` (`runtime.rs`) from
   `Chat.options`.
4. The model chip and `/model` emit `ChooseModel` (`native_view.rs`), which the
   workspace handles by opening `Panel::Settings` for that chat.

## Verify

- `npm run gpui:test` (and the `cargo test` run of `workspace_test.rs`) renders
  Customize > Models during the workspace selftest.
- `cargo test -p zest-core --lib prefs::tests` and `remembered` cover the
  sticky state and the legacy migration.

## Pitfalls

- The desktop app passes remembered values through the strict `with_model` /
  `with_effort` path, not `with_remembered_options`. A remembered model or
  effort the provider no longer offers fails the turn instead of being dropped;
  pick another model in Customize > Models to clear it.
- `prefs.rs` migrates only project-level legacy `last-model` / `last-effort` /
  `last-thread-id`, and only when the sticky thread belongs to the active
  provider. Reading user-level legacy files once leaked a Codex model into the
  Claude slot and made Claude unselectable.
- Current choices include GPT-6.2 Sol (default), GPT-6.2 Astra, GPT-6.1 Terra,
  GPT-6.1 Sol, GPT-6 Astra, GPT-6 Sol and GPT-6 Luna on both Codex transports,
  and Claude
  Opus 5.5, Fable 5.2, Sonnet 5.5, Haiku 5.5, Mythos 5.2 and Fable 5.1 on the
  native API and as Claude Code's built-in ids (its bare aliases are offered
  only when configured; see [cli-agent-providers](cli-agent-providers.md)). A
  saved or configured default alias that is no longer offered migrates to its
  explicit successor; a configured alias still in the catalogue is retained.
  The builtin Codex list no longer offers retired GPT-5.4 or GPT-5.5 choices;
  a configured model allow-list remains authoritative.
