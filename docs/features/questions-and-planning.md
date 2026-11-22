---
title: Questions and planning
summary: The agent can pause a turn to ask one structured question, and Plan mode runs the plan skill without writes.
paths:
  - crates/core/src/tools/question.rs
  - crates/desktop/src/interaction.rs
  - crates/desktop/src/native_view.rs
  - crates/desktop/src/native.rs
tests:
  - crates/desktop/src/workspace_test.rs
verify:
  - cargo test -p zest-core --lib -- tools::question
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- workspace_test::
  - npm run gpui:test
---

# Questions and planning

## Behavior
- `ask_user` takes `question` (max 2000 chars), optional `choices` (max 8,
  unique case-insensitively, each max 200 chars), `multiple` (requires choices),
  and `placeholder`. Invalid input fails as a tool error before anything is shown.
- The turn pauses until the user answers; the answer text becomes the tool
  result (never a synthetic user message). An empty answer is refused. Only one
  `ask_user` per model batch; a second one errors. The question is resolved
  before any other call in that batch runs.
- Only the parent chat of a Zest-owned agent loop gets `ask_user` (not workers,
  not CLI-owned providers, not headless callers, whose `DenyQuestioner` errors).
  CLI-owned providers can still ask through their own question requests, which
  use the same card.
- The question appears in a card above the composer with the prompt and one
  button per choice. A single-choice click answers at once. With `multiple`,
  clicks toggle choices (marked with ✓) and "Submit choices" sends them, joined
  with newlines. The "Type an answer and press Enter" field sends a free-text
  answer. "Dismiss" answers nothing and the tool fails with "Question dismissed".
- Approval and question cards are shown one at a time, in the order they
  arrived. When the turn ends, any card still open is dropped, which denies it.
- Plan mode (Customize > Models > Tool permissions > `plan`; the status line
  shows "Plan") sends every message through the `plan` skill unless the message
  names a command itself, and refuses writes and commands. If no `plan` skill
  is installed the message passes through unchanged.
- The desktop app does not show a plan questionnaire, a document card for plan
  answers, or a **Build plan** button; a plan answer renders like any other
  answer, and the user leaves Plan mode in Customize before asking for the build.

## How it works
1. `register_question_tool` (tools/mod.rs) is called in runtime.rs when a
   questioner is supplied and the chat is the parent; `INTERACTIVE_QUESTION_SYSTEM`
   (prompt.rs) is appended to the system prompt. The offline demo registers it
   directly in `native.rs` `Chat::agent`.
2. `AskUser::prepare` validates with `parse_question_input`; `run` is never used.
   `Agent::run_question_call` (agent.rs) calls `questioner.prepare`, emits
   `StreamEvent::QuestionNeeded`, then waits on `questioner.answer` or cancel.
3. Desktop: `interaction.rs` `Interactions` is the chat's `Questioner`,
   `Approver` and `ProviderInteractionHost`. `ask` creates a one-shot reply
   channel, then sends `EvKind::Card` with the request; dropping the card
   (or a closed event channel) answers `Deny`.
4. UI: `main.rs` `Root::apply` queues the card in `Root::cards` and emits
   `ChatAttention::Question` for [notifications](notifications.md).
   `native_view.rs` `native_controls` draws the front card; `Root::respond`
   pops it and sends `Response::Answer` or `Response::Deny`. Free text from the
   `decision_input` composer answers only when the front card is a question.
5. Plan mode: `native.rs` `Chat::run` uses `expand_command` first and, when no
   command matched and `policy.mode() == ApprovalMode::Plan`, falls back to
   `zest_core::expand_command_as(text, skills, "plan")`.

## Verify
- `npm run gpui:test` (and `workspace_test::` in a GPUI test window) sends
  `Demo: question` in the offline demo, checks the card offers "First" and
  "Second", answers "Second", and checks the answer reached the saved wire
  history.
- By hand: `npm run gpui -- --demo`, send `Demo: question`, pick a choice or
  type an answer; the turn finishes.

## Pitfalls
- A question's reply channel must exist before the card event is sent, or a
  fast click races it (same rule as approvals); `Interactions::ask` creates it
  first.
- `Root::respond` always answers the front card. Keep cards in arrival order so
  an answer never reaches a different request.
