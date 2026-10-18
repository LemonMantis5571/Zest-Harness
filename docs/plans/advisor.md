# Advisor: a stronger model the working model can consult

Status: steps 1, 2, and 5 (the Settings picker, moved up because nobody should have to edit `zest.toml`) built and tested; not yet tried against live providers. Step 2 consults only when no new approval is needed; see the feature entry.
What exists is described in [docs/features/advisor.md](../features/advisor.md).

## Goal

Let a cheap, fast model do the work in a chat or a delegated task, and consult a
stronger model at the moments that decide the outcome: before committing to an
approach, when an error keeps coming back, and before declaring the task done.
It must work with any provider whose agent loop Zest runs, not only Anthropic.

This is the pattern behind Claude Code's `--advisor` flag and Anthropic's
`advisor_20260301` server tool. Anthropic reports Haiku with an Opus advisor at
41.2% on BrowseComp against 19.7% alone, and Sonnet with an Opus advisor scoring
higher than Sonnet alone at lower cost per task. Zest can offer the same thing
across providers: DeepSeek doing the work with an Opus or GPT advisor, for
example.

## Non-goals

- **Delegation stays as it is.** Feature cards, isolated worktrees, the
  mandatory reviewer, and Zest-run acceptance checks are the path for large
  tasks. The advisor is for the moment inside a single turn when the working
  model needs judgment it does not have.
- No orchestrator model, no fan-out, no automatic delegation.
- The advisor never calls tools, edits files, or talks to the user.

## How one consultation works

1. A parent runtime whose provider Zest drives registers `consult_advisor`
   next to the other tools. To the working model it is an ordinary function
   with one optional input, `question`.
2. Before the tool runs, the agent hands it the conversation through the
   existing `uses_context` / `update_context` hook. That copy already has
   sensitive tool output redacted (`redact_sensitive_staged`), the same copy
   delegation receives.
3. The tool renders the conversation as **quoted text**, not as structured
   messages. Anthropic's server tool does the same ("the executor's full
   transcript as quoted context"), and it removes every cross-provider
   problem at once:
   - no tool definitions are needed, so no provider rejects the request for
     carrying tool history without tools;
   - the pending `consult_advisor` call, which has no result yet, is simply
     left out (OpenAI-format endpoints reject an unanswered tool call);
   - thinking and reasoning blocks are dropped (they are provider-private and
     signed), and images become a placeholder;
   - tool results are marked as evidence, not instructions.
4. If the rendering is larger than the advisor's budget, the first user
   message is kept and the oldest middle sections are dropped first, with a
   marker saying how many were left out.
5. The request goes to the advisor's provider with no tools, a fixed Zest
   system prompt, and a bounded `max_tokens`. Any native client works: the
   advisor's provider is resolved from the same provider registry as the chat.
6. The advice comes back as the tool result. The working model continues in the
   same turn. A failure (timeout, rate limit, no answer) is a tool error with a
   short reason, so the working model carries on without advice, as with
   Anthropic's tool.

## Consent and the credential boundary

When the advisor's provider is the chat's provider, nothing leaves that the
user is not already sending there, and the tool is `Read` risk: no card.

When it is a different provider, the whole conversation goes to a second
company. A project `zest.toml` can define providers, so without a gate a
repository could route every chat transcript to an endpoint of its choosing.
The tool is therefore `Exec` risk with the advisor's provider id as the
approval target: the first consultation shows a card, and "Allow for session"
covers the rest of the session for that provider only. Bypass mode allows it,
and Plan mode blocks it like any other gated call (see open questions).

## Limits

- `max_uses` per user request (default 3). Counting resets when a new user
  message starts a turn. Calls past the cap return a tool error telling the
  model to continue with what it has.
- `max_tokens` per consultation (default 4096; reasoning and text together on
  providers that think).
- `timeout_secs` per consultation (default 120).

## Usage and display

- Every consultation is recorded in the ledger under the advisor's provider and
  model, with its own task trace (`advisor`), so `zest usage` and the desktop
  meter show advisor spend separately from the working model.
- The transcript row reads "Ask advisor" with the advice as its summary; the
  expanded row shows the advice text.

## Configuration

```toml
[advisor]
provider = "anthropic"      # a provider id from [providers]
model = "claude-opus-5-5"   # optional: the provider's default model otherwise
effort = "high"             # optional
max_uses = 3                # per user request
max_tokens = 4096
timeout_secs = 120
```

`Config::lint` reports an unknown provider, a provider that runs its own agent
loop, and limits out of range. An advisor that cannot be loaded (no key,
unknown model) is skipped with a warning; the chat still starts.

## Provider coverage

| The chat runs on | Step 1 |
| --- | --- |
| Anthropic API | Yes |
| OpenAI API, DeepSeek, any OpenAI-compatible endpoint | Yes |
| ChatGPT sign-in (`codex_oauth`) | Yes |
| Claude Code | No. Pass `--advisor` through in step 4 |
| Codex CLI, Cursor | No. They run their own loop; step 4 offers the tool over Zest's MCP server |

The advisor itself can be any provider Zest drives, in any direction.

## Steps

1. **`consult_advisor` for parent chats on native providers.** Config, lint,
   transcript rendering, the tool, registration, consent, ledger records, a
   transcript row label, `zest.toml.example`, feature docs, and tests.
2. **Harness-triggered consultations.** Models other than Claude are not
   trained to call an advisor, so Zest also consults on its own:
   - before ending a turn that changed files (at most once per turn);
   - after two failed commands or tests in a row.

   If the advice finds a problem, Zest appends it as a labelled message and
   gives the working model one more round. Configurable, on by default only
   when an advisor is configured.
3. **Role defaults.** `[delegation]` worker and reviewer models, used by
   `delegate_feature` and the Workbench form when a card names none. Native
   delegated workers get the advisor too, with the same limits.
4. **CLI providers.** Pass `--advisor <model>` to Claude Code when the advisor
   is an Anthropic model. Offer `consult_advisor` to Codex and Cursor through
   the MCP server planned in `zest-mcp-runtime.md`.
5. **Desktop picker.** Choose the advisor in Settings, with the consent copy,
   instead of editing `zest.toml`.
6. **Anthropic fast path.** When both models are Anthropic, attach the
   `advisor_20260301` server tool instead of a second request. Needs server
   tool definitions in `ToolDef`, a list of beta headers, and
   `usage.iterations` parsing. An optimisation only.

## Evaluation before making it a default

Run a fixed set of tasks headless with `zest run --jsonl` three ways: the
working model alone, the working model with the advisor, and the advisor model
alone. Compare acceptance checks passed and cost per task. Turn on the step 2
triggers by default only if the numbers hold.

## Risks and open questions

- **Plan mode blocks a cross-provider consult**, because every gated risk is
  refused there. Planning is where advice is most useful. Options: a dedicated
  egress risk that Plan mode asks about instead of refusing, or treating a
  session grant as valid in Plan mode for this tool only.
- **Cost.** Each consultation re-reads the whole conversation. Providers with
  automatic prefix caching (OpenAI, DeepSeek) reuse the stable rendered prefix;
  Anthropic needs a cache breakpoint on the advisor message.
- **Advisor context window.** A smaller window than the working model's means
  trimming; step 1 trims by characters, later steps can reuse compaction.
- **Prompt injection.** Tool output inside the transcript can contain
  instructions. The advisor prompt says it is evidence, and the advice goes back
  to the working model as a tool result, never as a user message (until step 2,
  which labels it).
- **Latency.** The advisor does not stream into the turn; the row shows it is
  waiting.
