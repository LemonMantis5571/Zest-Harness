---
title: Usage, cost and provider quota
summary: See tokens, estimated cost and cache reuse across Zest and your coding CLIs, and check live limits or balance reported by each provider.
paths:
  - crates/core/src/usage.rs
  - crates/core/src/quota.rs
  - crates/core/src/pricing.rs
  - crates/core/src/rates.rs
  - crates/core/src/transcripts.rs
  - crates/core/src/profile.rs
  - crates/desktop/src/workspace.rs
  - crates/desktop/src/native.rs
tests:
  - crates/core/tests/token_efficiency_eval.rs
verify:
  - cargo test -p zest-core --lib usage::tests
  - cargo test -p zest-core --test token_efficiency_eval
  - cargo test -p zest-core --lib quota::tests
  - cargo test -p zest-core --lib pricing::tests
  - cargo test -p zest-core --lib rates::tests
  - cargo test -p zest-core --lib transcripts::tests
  - cargo test -p zest-core --lib profile::tests
  - npm run gpui:test
---

# Usage, cost and provider quota

Provider-facing rules and sources are documented in [QUOTA.md](../QUOTA.md).

## Behavior

- Three things are never merged: local usage (what Zest metered), rate-limit
  headroom (short-window throughput from response headers), and account
  quota/balance (only from a provider check). Local counts are never shown as
  "remaining", "balance" or "quota"; a failed check is an error or unavailable
  state, never 0.
- Every completed turn is recorded in `<data dir>/zest/usage.json` (outside the
  project), per provider, per model, and per local day (400 days kept). The turn
  is billed to the requested model unless the endpoint served a genuinely
  different one. Write failures never fail a turn. The offline demo uses
  `.zest/demo-usage.json` in its temporary project instead.
- Task traces are off unless `[usage] task_traces = true`. When on, each local
  task (turn, compaction, side conversation, delegation worker or reviewer)
  gets a content-free `TaskUsageRecord` in `usage.json`: provider rounds, token
  counts, latency, tool names and outcomes, never prompts, tool bodies, or
  paths. Kept 30 days and capped (2,000 records, 256 rounds and 1,024 tool
  records per task, oldest dropped first). `zest usage --tasks` prints them. A
  malformed trace is dropped on read instead of breaking the spend history.
- New traces include `runId`: the desktop turn ID (`Agent::set_next_run_id`
  in `native.rs`), the headless CLI's emitted run ID, or a fresh task ID for
  callers without a lifecycle record. Side conversations inherit it. Workers
  and reviewers share their delegation run; when a chat dispatches a job, the
  tool's job ID joins them to that chat run in either scheduling order,
  including maintenance descendants. Older traces without a run ID remain
  readable. `zest usage --tasks` shows both IDs. Starting a new host run clears
  stale correlation left by a dropped turn, so a side question during
  preparation cannot join an earlier aborted run.
- Day boundaries follow `usage::set_local_offset_minutes`. The desktop app sets
  it from the OS clock at launch, so its days are local; the CLI does not set
  it and buckets days in UTC.
- Headroom is overwritten only when a provider actually reported limits.
- Customize > Usage (also the sidebar's gauge button) lists, per provider in
  the ledger, measured requests, input tokens and output tokens, or "No
  measured usage yet." The text has a Copy button. The desktop app does not
  show estimated cost, the 7 / 30 / 90-day report, per-day or per-model
  breakdowns, CLI transcript totals, or cache metrics; use `zest usage` for the
  cost report. Your profile has the activity heatmap (see
  [settings-and-appearance](settings-and-appearance.md)).
- Cost (CLI report) is an estimate at published API rates, labelled by
  source: provider reported, model priced, mixed, or unpriced. A model with no
  rate stays unpriced (never zero); a missing cache rate bills at the input
  rate; bare family names like `opus` / `sonnet` are never priced. Totals merge
  Zest's ledger with Claude Code and Codex CLI transcripts, reported under
  `claude-cli` / `codex-cli` so Zest never claims turns it did not send.
- Price lookup order: `provider/model` then `model` in the user's override file
  `<data dir>/zest/prices.toml`, then the cached LiteLLM catalogue.
- The rate catalogue is fetched at most once per 24 h, never during a turn or
  while rendering a report.
- **Check provider quota** (Customize > Usage, not in the demo) runs one check
  on each click and replaces the text with one block per configured provider:
  `provider · detail`, then any balance (`<amount> <currency> balance`) and
  rate-limit windows (`<label>: N% used`). A failed check shows the error.
  Results are not cached and there are no bars.
- Live checks by kind: `codex_cli` → `codex app-server`
  `account/rateLimits/read`; `codex_oauth` → ChatGPT `wham/usage` with the stored
  session; `claude_code` → Claude Desktop's `plan-usage-history.json` (stale
  after 24 h); `openai_compatible` on `https://api.deepseek.com` → `/user/balance`.
  Anthropic API, Cursor and other endpoints report "unavailable" with a reason.
  No request ever goes to a guessed account URL.

## How it works

1. `Agent` (`agent.rs`) and `btw.rs` call `Ledger::record(provider, billed_model,
   completion)`; external workers use `Ledger::record_external`. The desktop
   passes one shared ledger (`Project.ledger`, loaded once per process with
   `Ledger::load`) to every chat and the delegation coordinator through
   `RuntimeBuilder::with_ledger`.
2. Usage tab: `Workspace::page(Panel::Usage)` (`workspace.rs`) formats
   `Ledger::snapshot().providers` into the panel's report text.
3. Quota: `Workspace::check_quota` runs core `fetch_provider_quotas(config)` on
   the Tokio executor. It checks every configured provider concurrently
   (`join_all`, 8 s timeouts) and returns them in config order. A result that
   arrives after the user left the panel is dropped (`report_epoch`).
4. CLI report: `zest usage` → `rates::refresh(false)`, then
   `transcripts::scan(30)` + `ledger.report(30, &Prices::load().with_catalog(..),
   Some(&scan))`.
   `transcripts.rs` de-duplicates Claude Code records by message/request id,
   drops repeated Codex `token_count` events, and subtracts cached tokens from
   Codex `input_tokens`.

## Verify

- `npm run gpui:test` renders Customize > Usage during the workspace selftest.
- `cargo test -p zest-core --test token_efficiency_eval` runs the eval's offline
  tests. Its live run makes paid requests and is ignored by default: set
  `ZEST_TOKEN_EVAL=1`, `ZEST_TOKEN_EVAL_PROVIDER`, and a spend cap in
  `ZEST_TOKEN_EVAL_LIMIT_USD` (optionally `_MODEL`, `_EFFORT`, `_VARIANTS`),
  then pass `-- --ignored`. Tasks live in `tests/fixtures/token_efficiency_tasks.json`.
- CLI: `zest usage` prints the ledger, headroom and a 30-day cost with coverage.
- `quota::tests` cover response parsing, DeepSeek host gating, the Claude Desktop
  cache and ordering without network.

## Pitfalls

- The desktop Usage tab reads the in-memory ledger, loaded when the first
  project window opened. Turns from the CLI or another Zest process after that
  appear only after restarting the app; `zest usage` reads the file each time.
- OpenAI and Codex report cache reads but not writes, so "no writes" is not
  "nothing cached".
- A hand-edited `prices.toml` with a syntax error falls back to no overrides, not
  "everything is free".
- `ZEST_CODEX_COMMAND` points the Codex quota check at a standalone CLI when
  Windows resolves `codex` to a Store app.
