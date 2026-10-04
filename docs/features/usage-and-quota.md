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
  - crates/desktop/ui/src/components/UsageScreen.tsx
  - crates/desktop/ui/src/components/AgentQuotaButton.tsx
  - crates/desktop/ui/src/components/ProfileScreen.tsx
  - crates/desktop/ui/src/lib/quotaCache.ts
  - crates/desktop/ui/src/lib/quotaGauges.ts
  - crates/desktop/ui/src/lib/heatmap.ts
  - crates/desktop/ui/src/lib/cacheMetrics.ts
tests:
  - crates/desktop/ui/src/lib/quotaCache.test.ts
  - crates/desktop/ui/src/lib/quotaGauges.test.ts
  - crates/desktop/ui/src/lib/heatmap.test.ts
  - crates/desktop/ui/src/lib/cacheMetrics.test.ts
  - crates/core/tests/token_efficiency_eval.rs
verify:
  - cargo test -p zest-core --lib usage::tests
  - cargo test -p zest-core --test token_efficiency_eval
  - cargo test -p zest-core --lib quota::tests
  - cargo test -p zest-core --lib pricing::tests
  - cargo test -p zest-core --lib rates::tests
  - cargo test -p zest-core --lib transcripts::tests
  - cargo test -p zest-core --lib profile::tests
  - npm run ui:test
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
  different one. Write failures never fail a turn.
- Task traces are off unless `[usage] task_traces = true`. When on, each local
  task (turn, compaction, side conversation, delegation worker or reviewer)
  gets a content-free `TaskUsageRecord` in `usage.json`: provider rounds, token
  counts, latency, tool names and outcomes, never prompts, tool bodies, or
  paths. Kept 30 days and capped (2,000 records, 256 rounds and 1,024 tool
  records per task, oldest dropped first). `zest usage --tasks` prints them. A
  malformed trace is dropped on read instead of breaking the spend history.
- New traces include `runId`: the durable desktop turn ID, the headless CLI's
  emitted run ID, or a fresh task ID for callers without a lifecycle record.
  Side conversations inherit it. Workers and reviewers share their delegation
  run; when a chat dispatches a job, the tool's job ID joins them to that chat
  run in either scheduling order, including maintenance descendants. Older
  traces without a run ID remain readable. `zest usage --tasks` shows both IDs.
  Starting a new host run clears stale correlation left by a dropped turn, so
  a side question during preparation cannot join an earlier aborted run.
- Day boundaries use the webview's timezone, sent once at startup
  (`set_local_offset`); the CLI stays on UTC.
- Headroom is overwritten only when a provider actually reported limits.
- Usage screen: 7 / 30 / 90 days (default 30), cost or tokens, by model or by
  day. Days with no traffic are zeroes, not gaps. Totals merge Zest's ledger with
  Claude Code and Codex CLI transcripts, reported under `claude-cli` /
  `codex-cli` so Zest never claims turns it did not send. Escape returns to chat.
- Cost is an estimate at published API rates, labelled by source: provider
  reported, model priced, mixed, or unpriced. A model with no rate stays
  unpriced (never zero); a missing cache rate bills at the input rate; bare
  family names like `opus` / `sonnet` are never priced. The rates' age is shown.
- Price lookup order: `provider/model` then `model` in the user's override file
  `<data dir>/zest/prices.toml`, then the cached LiteLLM catalogue.
- The rate catalogue is fetched at most once per 24 h (Refresh forces it), never
  during a turn or while rendering a report.
- Agent quota panel (sidebar footer): checks run when the panel opens, are
  cached 5 minutes in the UI, and Refresh forces a new check. On failure it
  keeps the last result and says so. Bars appear only where the provider gave a
  denominator (rate-limit windows, spend limit, reported percentage or request
  budget); a raw balance gets text, no bar. Tone: ≤10% critical, ≤25% low.
- Live checks by kind: `codex_cli` → `codex app-server`
  `account/rateLimits/read`; `codex_oauth` → ChatGPT `wham/usage` with the stored
  session; `claude_code` → Claude Desktop's `plan-usage-history.json` (stale
  after 24 h); `openai_compatible` on `https://api.deepseek.com` → `/user/balance`.
  Anthropic API, Cursor and other endpoints report "unavailable" with a reason.
  No request ever goes to a guessed account URL.
- Profile screen: activity heatmap (no record = tone 0, a recorded zero = 1,
  quartiles of the busiest day = 2–5) and a cache tile from Zest's own ledger.

## How it works

1. `Agent` (`agent.rs`) and `btw.rs` call `Ledger::record(provider, billed_model,
   completion)`; external workers use `Ledger::record_external`.
2. Usage screen: `UsageScreen` → `usage_report(days)` (`crates/desktop/src/lib.rs`,
   clamps to 1..400, runs on a blocking thread) → `transcripts::scan(days)` +
   `Ledger::load().report(days, &Prices::load(), Some(&scan))`. Then
   `refresh_rates(force)` → `rates::refresh`; the report is re-read only if
   `fetchedAt` changed. "Open prices file" → `open_prices_file`.
3. `transcripts.rs` de-duplicates Claude Code records by message/request id,
   drops repeated Codex `token_count` events, and subtracts cached tokens from
   Codex `input_tokens`.
4. Quota panel: `AgentQuotaButton` → `createProviderQuotaLoader` (`quotaCache.ts`)
   → `provider_quota` → core `fetch_provider_quotas(config)`, which checks every
   configured provider concurrently (`join_all`, 8 s timeouts) and returns them in
   config order. Rows also use `usage_snapshot` headroom; `quotaGauges` decides
   what can be drawn.
5. Profile: `ProfileScreen` → `profile_stats` (`profile::derive` over thread files
   and ledger daily buckets) and `usage_snapshot` → `cacheMetrics` /
   `cacheVerdict`; cells coloured by `heatmapTone`. The usage screen's cache line
   uses `cacheWindowHint`.

## Verify

- Fixture: `/?fixture=1` provides synthetic `usageSnapshot`, `providerQuota` and
  `usageReport` data for the panel and screens.
- `cargo test -p zest-core --test token_efficiency_eval` runs the eval's offline
  tests. Its live run makes paid requests and is ignored by default: set
  `ZEST_TOKEN_EVAL=1`, `ZEST_TOKEN_EVAL_PROVIDER`, and a spend cap in
  `ZEST_TOKEN_EVAL_LIMIT_USD` (optionally `_MODEL`, `_EFFORT`, `_VARIANTS`),
  then pass `-- --ignored`. Tasks live in `tests/fixtures/token_efficiency_tasks.json`.
- CLI: `zest usage` prints the ledger, headroom and a 30-day cost with coverage.
- `quota::tests` cover response parsing, DeepSeek host gating, the Claude Desktop
  cache and ordering without network.

## Pitfalls

- `usage_report` reads the ledger from disk on every call so other windows and
  CLI runs are included; do not switch it to the in-memory ledger.
- Cache percentages differ by screen: usage screen totals include CLI
  transcripts, the profile tile is Zest-only. OpenAI and Codex report cache reads
  but not writes, so "no writes" is not "nothing cached".
- A hand-edited `prices.toml` with a syntax error falls back to no overrides, not
  "everything is free".
- `ZEST_CODEX_COMMAND` points the Codex quota check at a standalone CLI when
  Windows resolves `codex` to a Store app.
