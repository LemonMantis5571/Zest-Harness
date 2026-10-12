---
title: Advisor
summary: The chat model can consult a stronger model mid-turn with consult_advisor, on any provider Zest drives, behind an approval when the advisor is on a second provider.
paths:
  - crates/core/src/advisor.rs
  - crates/core/src/tools/consult_advisor.rs
  - crates/desktop/ui/src/components/AdvisorSettings.tsx
  - docs/plans/advisor.md
tests:
  - crates/desktop/ui/src/lib/mcpDisplay.test.ts
  - crates/desktop/ui/e2e/advisor-settings.spec.ts
verify:
  - cargo test -p zest-core --lib -- advisor
  - cargo test -p zest-core --lib -- agent::tests::advisor_reviews
  - cargo test -p zest-core --lib -- config::tests::an_advisor_section_parses_with_defaults_and_lints_clean
  - cargo test -p zest-core --lib -- runtime::tests::the_advisor_joins_parent_chats_only_and_a_broken_one_is_a_warning
  - npm run ui:test
  - npm run ui:e2e -- advisor-settings
  - cargo test -p zest-core --lib -- config_edit::tests::set_advisor
---

# Advisor

Design, later steps, and open questions: [docs/plans/advisor.md](../plans/advisor.md). This entry describes what is built: the tool (step 1), the Settings picker (step 5), and Zest's own consultations (step 2).

## Behavior
- Chosen in Settings > Advisor (also in the sidebar account menu): Off, or one of the configured providers Zest drives, with its model, consultations per request (1, 2, 3, or 5), and the "Review automatically" switch (on by default). Providers that are not keyed or signed in are listed but disabled; the chat's own provider is marked "same as this chat". Every choice saves at once and rebuilds the session so the open chat gains or loses the tool. A second provider shows the note that Zest asks before sending the conversation there; a chat on a CLI provider shows that it cannot consult the advisor.
- Stored as `[advisor]` in `zest.toml` (the project file when one exists, otherwise the user file): `provider` (a provider id), optional `model` (the provider's default otherwise) and `effort`, `max_uses` per user request (default 3, 1 to 20), `max_tokens` per consultation (default 4096, 1024 to 64000), `timeout_secs` (default 120, 1 to 600), `auto_review` (default true). Without the section nothing is registered.
- Parent chats on a provider Zest drives (Anthropic API, OpenAI-compatible, ChatGPT sign-in) get the `consult_advisor` tool and a short system-prompt paragraph about when to use it. Chats on Claude Code, Codex CLI, or Cursor do not, and neither do delegated workers or reviewers.
- The advisor provider must also be one Zest drives. An advisor that cannot load (unknown provider, CLI provider, model not offered, limits out of range) is skipped with the session warning "The advisor is off for this chat: …"; the chat still starts. `Config::lint` reports the same problems.
- The tool takes an optional `question`. The advisor receives the conversation as quoted text with a fixed system prompt and no tools, and its reply returns to the chat model as the tool result.
- Same provider as the chat: `Read` risk, no card. Different provider: `Exec` risk. The card reads "Share this chat with the advisor?" and names `provider · model`; "Allow for session" trusts that pair for the session. Bypass mode allows it; Plan mode refuses it like any gated call.
- Past `max_uses` in one user request, a timeout, a provider error, or an empty reply is a tool error ending in "Continue without it", so the chat model carries on.
- Zest also consults on its own when `auto_review` is on, at most once each per turn: before a turn in which a write tool succeeded ends (asking for exactly `APPROVE` when nothing is missing), and right after two tool calls in a row failed (a tool error, or a command that exited non-zero; the advisor's own calls neither count nor break the streak). Advice other than `APPROVE` is added as a user-role note starting `[Zest: ` that says it is from Zest, not the user; a finish review then gives the model one more round. These consultations count against `max_uses`, and run only when they need no new approval: same provider, an "Allow for session" grant for that provider and model, or Bypass mode. Otherwise they are skipped and the model can still call the tool, which shows the card.
- The transcript row reads "Ask advisor · provider · model"; expanding it shows the advice (up to 4000 characters). Spend is recorded under the advisor's provider and model, with an `advisor` task trace when traces are on.

## How it works
0. Settings: `AdvisorSettings.tsx` renders `advisorSettings()`; the Tauri `advisor_settings` command lists configured, non-CLI providers through `configured_provider_view` and reports the `[advisor]` lint issue, if any. `set_advisor` calls `config_edit::set_advisor`, which keeps `effort`, `max_tokens`, and `timeout_secs` and refuses an unknown provider, a CLI provider, or limits out of range without touching the file.
1. `RuntimeBuilder::build` (`runtime.rs`) resolves `[advisor]` with `Advisor::resolve` (`advisor.rs`) against the same `ProviderRegistry` as the chat, before composing the prompt, then registers `ConsultAdvisor` (`tools/consult_advisor.rs`) after the HTML tools with the session ledger.
2. `ConsultAdvisor::uses_context` makes the agent call `update_context` with the redacted staged conversation (`redact_sensitive_staged` in `agent.rs`) right before the call runs, the same copy delegation receives.
3. `render_transcript` turns messages into `### User` / `### Agent` / `### Tool results` sections. It omits the last assistant message's tool calls (the pending ones, including this call), drops thinking and reasoning blocks, labels tool results with their tool name and failure, replaces images with `[image]`, keeps the head and tail of any block over 12,000 characters, and escapes `</transcript>`.
4. Over budget (the advisor model's context window at 3 characters per token, minus reply room, capped at 600,000 characters), the first section is kept and the oldest middle sections are dropped with a "[N earlier messages left out to fit]" marker.
5. `Advisor::request` builds a tool-free `TurnRequest` (`allow_tool_use: false`, thinking on, effort only when the model has efforts). `ConsultAdvisor::ask` sends it under `timeout_secs` and records `task_request_usage` and `Ledger::record` under the advisor provider.
6. `request_index` (user messages carrying text) keys the per-request counter, so tool-result rounds do not reset it and a new user message does.
7. Step 2 lives in the agent loop (`send_user_cancellable_inner` in `agent.rs`). The runtime gives the agent the same `Arc<ConsultAdvisor>` it registers as the tool (`Agent::with_advisor`), so both share one budget. A successful `ToolRisk::Write` outcome sets `wrote_files`; `is_error` or `ToolOutcome::failed` (set by `bash` on a non-zero exit) extends `failure_streak`. `Agent::harness_consult` checks `auto_review` and, across providers, `ApprovalPolicy::decide` for the `provider · model` target, then calls `ConsultAdvisor::review` (`Advisor::review_request`, which tells the advisor Zest is asking on the agent's behalf) and emits `ToolCallStart`/`ToolCallResult` for the row. The failure note is its own user message after the results message, because the OpenAI-compatible converter puts a results message's text before its tool messages. `request_index` ignores `[Zest: ` notes so they never reset the budget, and `render_transcript` labels them `### Zest`.
8. `ToolOutcome::summary` carries the advice to the UI in place of the 160-character digest; `ToolCallRow.tsx` and `approvalTitle` (`lib/mcpDisplay.ts`) label the row and card.

## Verify
- `cargo test -p zest-core --lib -- advisor` covers rendering (pending calls and thinking left out, failed results labelled, budget trimming, head-and-tail clipping, quote escaping, the request index) and the tool (a tool-free quoted request, ledger record under the advisor provider, the per-request limit and its reset, errors and timeouts as tool errors, approval only across providers).
- `cargo test -p zest-core --lib -- agent::tests::advisor_reviews` covers the finish review and its single extra round, an approving review, consent across providers (skipped in Manual, allowed in Bypass and after a session grant), the failure trigger with a results message left pure, and the switch.
- The config and runtime tests in the frontmatter cover parsing, lint, parent-only registration, and the warning for an advisor that cannot load.
- Live check: choose an advisor in Settings > Advisor, ask the chat to "consult the advisor about X", approve the card if it is on another provider, and expand the "Ask advisor" row. For step 2, ask for a small edit and watch for an "Ask advisor" row before the reply ends.

## Pitfalls
- Never replay the conversation to the advisor as structured messages: the pending call has no result (OpenAI-format endpoints reject it), and tool history without tool definitions is refused by some endpoints.
- The approval target is `provider · model`, also used as the row's `path`; it is not a file path and never matches a dirty path.
- Never append the advisor's note to the tool-results message: the OpenAI-compatible converter would send it before the tool messages and break call ordering.
- A project `zest.toml` can define providers, which is why a second-provider advisor stays behind the approval gate instead of trusting the config.
