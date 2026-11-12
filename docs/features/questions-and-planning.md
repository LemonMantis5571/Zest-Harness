---
title: Questions and planning
summary: The agent can pause a turn to ask one structured question, Plan mode runs the plan skill without writes, and a finished plan offers a Build button.
paths:
  - crates/core/src/tools/question.rs
  - crates/desktop/ui/src/components/NeedsInputCard.tsx
  - crates/desktop/ui/src/components/PlanningQuestionnaire.tsx
  - crates/desktop/ui/src/components/ui/questionnaire.tsx
  - crates/desktop/ui/src/components/CommandOutputCard.tsx
  - crates/desktop/ui/src/lib/planningQuestion.ts
  - crates/desktop/ui/src/lib/planActions.ts
  - crates/desktop/ui/src/lib/documentShape.ts
tests:
  - crates/desktop/ui/src/lib/planningQuestion.test.ts
  - crates/desktop/ui/src/lib/planActions.test.ts
  - crates/desktop/ui/src/lib/documentShape.test.ts
verify:
  - cargo test -p zest-core --lib -- tools::question
  - cargo test -p zest-desktop --lib -- question_hub
  - npm run ui:test
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
- The question appears in the anchored card above the composer ("Needs your
  input"). Pending approvals take precedence over a pending question.
  Multi-select answers are joined with newlines.
- Plan mode sends every message through the `plan` skill (unless the message
  names a skill itself) and refuses writes and commands. If no `plan` skill is
  installed the message passes through unchanged.
- A finished `/plan` answer that is short (<= 700 chars) and question-shaped is
  shown as a questionnaire whose answer is sent as a new message. A
  document-shaped answer renders in `CommandOutputCard` (title, copy/save,
  collapse).
- Only the newest finished, document-shaped plan offers **Build plan**, and only
  while nothing with text came after it. Build leaves Plan mode (restoring the
  mode used before Plan, else Auto) and sends "Build the plan. Delegate the steps
  that suit a configured external worker; build the rest here." If leaving Plan
  mode fails, nothing is sent.

## How it works
1. `register_question_tool` (tools/mod.rs) is called in runtime.rs when a
   questioner is supplied and the chat is the parent; `INTERACTIVE_QUESTION_SYSTEM`
   (prompt.rs) is appended to the system prompt.
2. `AskUser::prepare` validates with `parse_question_input`; `run` is never used.
   `Agent::run_question_call` (agent.rs) calls `questioner.prepare`, emits
   `StreamEvent::QuestionNeeded`, then waits on `questioner.answer` or cancel.
3. Desktop: `QuestionHub` (crates/desktop/src/lib.rs) holds turn-scoped waiters.
   The UI answers through `resolve_question(question_id, answer, thread_id)`,
   which resumes the same turn and records it in `ChatPersistence.interrupts`.
4. UI: `question_needed` is reduced in chatReducer.ts onto the message;
   ChatScreen finds `pendingQuestion` and renders `NeedsInputCard` ->
   `PlanningQuestionnaire` -> `Questionnaire` primitives (letter shortcuts for
   choices). App.tsx `onResolveQuestion` calls the backend.
5. Plan mode: turn.rs checks `policy.mode() == ApprovalMode::Plan` and uses
   `zest_core::expand_command_as(text, skills, PLAN_SKILL)`.
6. Plan UI: `planningQuestionFor` (planningQuestion.ts), `buildablePlanId`
   (planActions.ts), `looksLikeDocument` (documentShape.ts), and App.tsx
   `onBuildPlan` with `modeBeforePlanRef` (set in `onApprovalModeChange`).

## Verify
Unit tests above. Fixture mode: `?fixture=1&scenario=question`, send any
message, pick "safe" or "review" and press "Send answer"; the turn finishes.

## Pitfalls
- A question's waiter must be registered before the event is emitted, or a fast
  click races it (same rule as approvals).
- `planningQuestionFor` returns null while the message streams, for non-`plan`
  commands, and for long or document-shaped text; adjusting it changes both the
  questionnaire and the Build button.
- The Build prompt says "the plan"; the model builds the newest plan in the
  conversation, which is why only one button is offered.
