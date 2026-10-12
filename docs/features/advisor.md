---
title: Advisor
summary: The chat model can consult a stronger model mid-turn with consult_advisor, on any provider Zest drives, behind an approval when the advisor is on a second provider.
paths:
  - crates/core/src/advisor.rs
  - crates/core/src/tools/consult_advisor.rs
  - docs/plans/advisor.md
tests:
  - crates/desktop/ui/src/lib/mcpDisplay.test.ts
verify:
  - cargo test -p zest-core --lib -- advisor
  - cargo test -p zest-core --lib -- config::tests::an_advisor_section_parses_with_defaults_and_lints_clean
  - cargo test -p zest-core --lib -- runtime::tests::the_advisor_joins_parent_chats_only_and_a_broken_one_is_a_warning
  - npm run ui:test
---

# Advisor

Design, later steps, and open questions: [docs/plans/advisor.md](../plans/advisor.md). This entry describes what is built (step 1).

## Behavior
- Configured with `[advisor]` in `zest.toml`: `provider` (a provider id), optional `model` (the provider's default otherwise) and `effort`, `max_uses` per user request (default 3, 1 to 20), `max_tokens` per consultation (default 4096, 1024 to 64000), `timeout_secs` (default 120, 1 to 600). Without the section nothing is registered.
- Parent chats on a provider Zest drives (Anthropic API, OpenAI-compatible, ChatGPT sign-in) get the `consult_advisor` tool and a short system-prompt paragraph about when to use it. Chats on Claude Code, Codex CLI, or Cursor do not, and neither do delegated workers or reviewers.
- The advisor provider must also be one Zest drives. An advisor that cannot load (unknown provider, CLI provider, model not offered, limits out of range) is skipped with the session warning "The advisor is off for this chat: …"; the chat still starts. `Config::lint` reports the same problems.
- The tool takes an optional `question`. The advisor receives the conversation as quoted text with a fixed system prompt and no tools, and its reply returns to the chat model as the tool result.
- Same provider as the chat: `Read` risk, no card. Different provider: `Exec` risk. The card reads "Share this chat with the advisor?" and names `provider · model`; "Allow for session" trusts that pair for the session. Bypass mode allows it; Plan mode refuses it like any gated call.
- Past `max_uses` in one user request, a timeout, a provider error, or an empty reply is a tool error ending in "Continue without it", so the chat model carries on.
- The transcript row reads "Ask advisor · provider · model"; expanding it shows the advice (up to 4000 characters). Spend is recorded under the advisor's provider and model, with an `advisor` task trace when traces are on.

## How it works
1. `RuntimeBuilder::build` (`runtime.rs`) resolves `[advisor]` with `Advisor::resolve` (`advisor.rs`) against the same `ProviderRegistry` as the chat, before composing the prompt, then registers `ConsultAdvisor` (`tools/consult_advisor.rs`) after the HTML tools with the session ledger.
2. `ConsultAdvisor::uses_context` makes the agent call `update_context` with the redacted staged conversation (`redact_sensitive_staged` in `agent.rs`) right before the call runs, the same copy delegation receives.
3. `render_transcript` turns messages into `### User` / `### Agent` / `### Tool results` sections. It omits the last assistant message's tool calls (the pending ones, including this call), drops thinking and reasoning blocks, labels tool results with their tool name and failure, replaces images with `[image]`, keeps the head and tail of any block over 12,000 characters, and escapes `</transcript>`.
4. Over budget (the advisor model's context window at 3 characters per token, minus reply room, capped at 600,000 characters), the first section is kept and the oldest middle sections are dropped with a "[N earlier messages left out to fit]" marker.
5. `Advisor::request` builds a tool-free `TurnRequest` (`allow_tool_use: false`, thinking on, effort only when the model has efforts). `ConsultAdvisor::ask` sends it under `timeout_secs` and records `task_request_usage` and `Ledger::record` under the advisor provider.
6. `request_index` (user messages carrying text) keys the per-request counter, so tool-result rounds do not reset it and a new user message does.
7. `ToolOutcome::summary` carries the advice to the UI in place of the 160-character digest; `ToolCallRow.tsx` and `approvalTitle` (`lib/mcpDisplay.ts`) label the row and card.

## Verify
- `cargo test -p zest-core --lib -- advisor` covers rendering (pending calls and thinking left out, failed results labelled, budget trimming, head-and-tail clipping, quote escaping, the request index) and the tool (a tool-free quoted request, ledger record under the advisor provider, the per-request limit and its reset, errors and timeouts as tool errors, approval only across providers).
- The config and runtime tests in the frontmatter cover parsing, lint, parent-only registration, and the warning for an advisor that cannot load.
- Live check: add `[advisor]` pointing at a second configured provider, ask the chat to "consult the advisor about X", approve the card, and expand the "Ask advisor" row.

## Pitfalls
- Never replay the conversation to the advisor as structured messages: the pending call has no result (OpenAI-format endpoints reject it), and tool history without tool definitions is refused by some endpoints.
- The approval target is `provider · model`, also used as the row's `path`; it is not a file path and never matches a dirty path.
- A project `zest.toml` can define providers, which is why a second-provider advisor stays behind the approval gate instead of trusting the config.
