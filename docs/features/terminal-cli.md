---
title: Terminal CLI (zest)
summary: The zest binary: an interactive terminal chat with y/N approvals, auth and usage reports, a live doctor check, and zest run --jsonl for one machine-readable, deny-only agent turn.
paths:
  - crates/cli/src/main.rs
tests:
  - crates/cli/tests/run_e2e.rs
verify:
  - cargo test -p zest --bin zest
  - cargo test -p zest --test run_e2e
  - cargo run -p zest -- --help
  - cargo run -p zest -- run --help
  - cargo tree -p zest --edges normal
---

# Terminal CLI (zest)

`crates/cli` builds the `zest` binary. `zest serve` has its own page:
[zest-serve](zest-serve.md).

## Behavior

- **Startup.** A first argument of `-h`/`--help` prints help. Every other
  subcommand first calls `ensure_user_config` and `load_env`.
- **`zest`** (no subcommand) starts an interactive loop in the current
  directory.
  - The provider comes from `Config::find`. Effort comes from `ZEST_EFFORT`,
    default `high`.
  - The session has write and exec tools, plus `delegate_external` and
    `delegate_feature` when `[agents.*]` exists.
  - `PromptApprover` asks `[y/N]`. Only `y`/`yes` allows. Any other answer, or
    EOF, denies, including provider command and file-change requests.
  - `/btw [question]` opens a side conversation and `/back` discards it.
  - The loop does not save threads, and no coordinator runs in it. A
    `delegate_feature` card waits for the desktop or `zest serve` to ingest its
    receipt.
- **`zest auth`** prints the launch-picker providers (`detect_all`) and then
  any extra `[providers.*]` from `zest.toml`, each marked selectable (●) or
  unavailable (○).
- **`zest usage`** refreshes the rate catalog at most once a day. It then prints
  the ledger per provider (spent versus provider-reported headroom) and a
  30-day cost report with its coverage. `--tasks` adds recent per-task costs
  from the task traces, which are off unless `[usage] task_traces = true`
  ([usage-and-quota](usage-and-quota.md)).
- **`zest doctor --live`** runs one real read-only turn against `README.md` in
  the current directory, using the separate ledger `.zest/doctor-usage.json`.
  It then checks that text streamed, that `read_file` started and succeeded,
  that ledger requests went up, and that a saved thread reloads. `zest doctor`
  without `--live` prints help and exits 2. It spends quota and is never run in
  CI.
- **`zest run --jsonl`** runs one deny-only turn. The prompt comes after `--`,
  from other free arguments, or from stdin.
  - Options: `--jsonl` is required, and `--json` is accepted as an alias.
    `--provider`, `--model` and `--effort` are optional, and effort falls back
    to `ZEST_EFFORT` and then `high`.
  - stdout carries JSON lines only. Config lint warnings go to stderr.
  - `--usage-file PATH` uses a separate local ledger instead of the account
    ledger. This lets fixture tests and integrations keep their usage isolated.
    The trace setting still comes from `[usage] task_traces`.

### zest-jsonl-v1 events (one JSON object per line, `kind` first)

Every event includes the same `run_id`, also used by optional local task
traces. Each invocation gets a new ID; it contains no prompt or project path.

| kind | fields |
| --- | --- |
| `session` | `protocol:"zest-jsonl-v1"`, `provider`, `model`, `effort` (always the first line) |
| `text` / `thinking` | `text` (empty deltas are dropped) |
| `provider_activity` | `id`, `title`, `status` (activity inside a provider's own loop) |
| `tool_call_start` | `name`, `id` |
| `tool_call_update` | `name`, `id`, `metadata` |
| `tool_call_result` | `name`, `id`, `summary`, `isError`, `path`, `diff`, `metadata` |
| `approval_needed` | `approvalId`, `toolName`, `toolCallId`, `risk`, `path`, `summary`, `diff` |
| `approval_decision` | `approvalId`, `decision:"deny"` (from `JsonApprover`, for Zest-owned tools) |
| `question_needed` | `questionId`, `toolCallId`, `question`, `choices`, `multiple`, `placeholder` |
| `model_substituted` | `requested`, `served` |
| `done` | success; the last line |
| `error` | `message` (the provider's own wording when it gave one); the last line |

A denied tool comes back to the model as a failed `tool_call_result` ("user
denied permission ..."), and the turn carries on. `ResumeHandle` events are not
emitted.

Exit codes:

- `0` after `done`.
- `1` after `error`. stderr carries the same message.
- `1` with no JSONL at all for setup failures: a missing `--jsonl`, an empty
  prompt, an unknown option, a bad config, or a runtime that cannot be built.

## How it works

1. `main()` matches `std::env::args().nth(1)` against `auth`, `usage`,
   `doctor`, `run` and `serve`, then falls through to the interactive loop.
2. Every path builds a session with `RuntimeBuilder`. The interactive loop and
   `run` pass `DEFAULT_SYSTEM` and `enable_external_agents(true)`. `doctor`
   uses its own prompt and disables writes, exec and external agents.
3. `run_headless` streams through `emit_stream_json` and uses `JsonApprover`.
   It sets no `provider_interaction` host.
4. The interactive loop renders through `Renderer` and approves with
   `PromptApprover`, which also answers provider questions
   (`parse_question_answers`).

## Verify

- `cargo test -p zest --bin zest` runs the `parse_question_answers` unit tests,
  together with the `serve::mcp` response-size tests.
- `cargo test -p zest --test run_e2e` exercises the real binary against a local
  streaming provider. It checks tool rounds, denied writes, failed turns,
  matching run IDs on every event, and content-free saved traces. It spends no
  provider quota and uses a disposable usage ledger.
  The local provider fixture explicitly uses blocking accepted connections
  with read/write deadlines, including a delayed-request regression on Windows.
- For the protocol, run
  `cargo run -p zest -- run --jsonl -- "list the files here"`. This uses a real
  provider and quota. Check that the first line is `session` and the last is
  `done` or `error`.

## Pitfalls

- **No desktop dependency.** The CLI must not depend on `zest-desktop` or
  WebKit. The `cli-headless` job in `.github/workflows/linux-verify.yml` fails if
  `cargo tree -p zest --edges normal` mentions either one.
- **`run` never waits for a human.** Adding an interactive approver there
  would make CI and editor callers hang.
- **Conversations are not saved.** The interactive loop never writes to
  `ThreadStore`, so a terminal conversation ends with the session (the README
  says so). `delegate_feature` cards created from the terminal use parent
  thread id `coordinator`.
