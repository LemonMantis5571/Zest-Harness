---
title: Delegation (feature cards)
summary: Hand a scoped task to a separate native or external worker, have a fresh reviewer check it in its own worktree, then apply the diff yourself.
paths:
  - crates/core/src/delegation.rs
  - crates/core/src/delegated_worker.rs
  - crates/core/src/orchestration.rs
  - crates/core/src/handoff.rs
  - crates/core/src/tools/delegate_feature.rs
  - crates/core/src/tools/external_agent.rs
  - crates/core/src/tools/isolated_workspace.rs
  - crates/core/src/worker_activity.rs
  - crates/coordinator/src/scheduler.rs
  - crates/desktop/src/workspace/board.rs
  - crates/desktop/src/native.rs
tests:
  - crates/core/tests/fixtures/external_agent_fixture.rs
  - crates/desktop/src/workspace/board/selftest.rs
verify:
  - cargo test -p zest-core --lib delegation
  - cargo test -p zest-core --lib delegated_worker
  - cargo test -p zest-core --lib orchestration
  - cargo test -p zest-core --lib handoff
  - cargo test -p zest-core --lib tools::external_agent
  - cargo test -p zest-core --lib tools::isolated_workspace
  - cargo test -p zest-coordinator
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- offline_workspace_smoke
  - npm run gpui:test
---

# Delegation (feature cards)

Queueing, locking and restart recovery are in
[delegation-coordinator](delegation-coordinator.md). The headless host is
[zest-serve](zest-serve.md).

## Behavior

The desktop's **Delegation** tab (under Workbench) uses the same coordinator as `zest serve`. It accepts scoped cards and separate worker and reviewer targets, reloads as worker events arrive, shows check output and review findings, and pages through the saved artifacts. Actions carry the revision displayed to the user. An existing coordinator lock permits browsing while the other process owns the queue. The native workspace self-test runs the fixture worker and reviewer and verifies the checkout before and after clicking apply.

- A feature card (title, objective, lane, scope, context, dependsOn,
  acceptanceChecks, worker, reviewer) becomes a job at
  `.zest/delegations/<jobId>.json`. Its artifacts are `worker.diff`,
  `worker-result.json`, `review-result.json`, `worker-transcript.jsonl` and,
  after a failed worker, `worker-partial.diff`, in `.zest/delegations/<jobId>/`.
- Every new card starts in `awaiting_approval`. No worker runs until an approval
  is recorded. That can be the desktop **Approve and run** button, `delegation_approve`,
  a trusted `zest serve`, or a dispatch receipt written by `delegate_feature`
  after the user answers `y`.
- Approval pins fingerprints of the card and of the resolved worker and reviewer
  targets (the config entry plus a non-secret credential reference). If the
  config or credential reference changes before the worker starts, the job goes
  back to `awaiting_approval` ("approved provider or worker configuration
  changed"). Any card edit clears the approval.
- Review cannot be turned off: `review_required: false` is rejected. The
  reviewer defaults to `sameAsWorker` but always runs as a fresh process or
  runtime in a fresh worktree.
- **Limits.** Every worker and reviewer process runs under two limits
  (`WorkerLimits`, enforced by `worker_activity::watchdog`): a total budget
  and a no-progress budget. External agents use `timeout_secs` and
  `idle_timeout_secs` (default 900 s) from their `[agents.<id>]` entry
  (`delegation_limits`); native provider workers get 60 minutes in total and
  15 minutes without a stream event. Any output line from a CLI, any ACP
  message and any native stream event counts as progress. A run that hits
  either limit is stopped and the job fails with "timed out after …" or
  "made no progress for … seconds". The no-progress clock starts when the
  process does, not while its worktree is prepared.
- **Worker transcript.** Every worker run, finished or not, leaves
  `worker-transcript.jsonl`: one `{"atMs","kind","text"}` object per line for
  its text, thinking, tool calls and results, diffs and errors, streamed
  chunks joined, each entry at most 8000 characters and the file at most
  1 MiB (`WorkerActivity::record`). External CLI and ACP events and native
  stream events all feed it. It is not an orchestration transcript of the
  reviewer.
- **Process trees.** An external worker or reviewer runs in a Windows job
  object (started suspended, without a console, then resumed) or its own
  Unix process group (`process_job`). A timeout, cancel or failure stops the
  whole tree, and closing the job at the end kills anything the CLI left
  running.
- **Partial work.** When a worker fails or is stopped, whatever it had
  changed in its worktree is saved as `worker-partial.diff` and the card
  error says so. It is evidence only: never reviewed, never applied by Zest,
  and it does not make the card reviewable. Read it through the artifact
  pager (`ALLOWED_ARTIFACTS`, MCP `delegation_artifact`).
- Workers never edit the user's checkout. Each runs in a temporary detached
  `git worktree` that holds the current dirty snapshot (tracked diff plus
  untracked files), with sensitive files removed and `.zest/` left out. What
  comes back is a diff.
- The worker's snapshot is a commit in the project's object store. The job
  records it (`worker_base`) and pins it as `refs/zest/delegations/<jobId>`
  (`pin_worker_base`) until the job is accepted or cancelled. The acceptance
  checks and every reviewer start from that commit plus the worker diff
  (`prepare_reviewer` with a base), so they see exactly the files the worker
  saw, even if the checkout changed meanwhile. A missing or malformed base
  falls back to snapshotting the current checkout.
- The worker diff must be non-empty, and every path in it must be inside the
  card `scope` (`.` means the whole project) and outside `.git`, `.zest` and
  sensitive paths. Otherwise the job fails.
- Zest runs the acceptance checks itself, in a separate worktree with the worker
  diff applied. It only runs a command that classifies as auto-run under
  `[tools.bash]`. Any other command is recorded as `skipped`. The reviewer sees
  these results as evidence and only returns a verdict and findings; the
  coordinator writes its own results into `review-result.json`, whatever the
  reviewer said about them.
- `ready_to_apply` needs all of the following: a review report that parses,
  decision `accepted`, evidence with `passed` for every required check, and no
  `blocking` finding. A reviewer that edits files sends the job to `blocked`,
  and its edits are discarded. A reviewer whose process fails or times out,
  or whose report does not parse, is replaced once by a fresh reviewer
  (`MAX_REVIEWER_ATTEMPTS`); the card inbox says so. The checks are not run
  again for that second reviewer.
- Apply checks the scope again and runs `git apply --check` before `git apply`.
  When the diff no longer applies as it is (the user changed lines around the
  worker's edits), `apply_worker_diff` replays the worker's change from its
  base onto a `git stash create` snapshot of the checkout in temporary
  worktrees (a cherry-pick) and applies the resulting patch, only if that
  merge is clean and touches only the reviewed paths. The card inbox says a
  three-way merge was used. Otherwise the job becomes `apply_conflict` and
  the checkout is left untouched, with no conflict markers. A successful apply leaves the job `accepted`. Zest saves an
  apply intent before Git touches the checkout. If the coordinator stops
  before recording the outcome, the card becomes `blocked` on reconciliation
  or the next Apply. Inspect the checkout against `worker.diff` before
  requesting a fresh attempt. This also applies to trusted daemons.
- Retry is always manual. `retry` moves a job back to `awaiting_approval`,
  clears its approval and takes a new workspace snapshot. Only one approval
  follows. The next worker attempt also gets the previous diff and review report
  as evidence.
- **Review again.** While a saved `worker.diff` has no verdict yet, the job
  records its hash (`reviewable_diff`). If the card then blocks or fails (the
  reviewer kept failing, edited files, or timed out), `canReviewAgain` is
  true and `review_again` (desktop **Review again**, MCP
  `delegation_review_again`) queues a fresh reviewer on that same diff with
  `review_only` set. The worker does not run, the approval still covers it,
  and a config change still sends the card back to `awaiting_approval`. A
  verdict (`ready_to_apply` or `changes_requested`) clears the hash, so a
  rejected diff cannot be sent to another reviewer.
- `accepted` and `cancelled` are final. Provider credentials are never written
  to a job. External worker processes run with secret-looking environment
  variables and the configured provider key names removed.
- A delegated call's chat row carries the worker's report
  (`ToolMetadata::delegation_summary`): its answer without the
  `[agent · model]` line or the workspace diff, cut at 600 characters, or
  "Delegated to <agent> · <model>" when the answer is empty. The model still
  receives the whole body.
- In the desktop chat, a `delegate_feature` call is an ordinary tool row
  whose title is the first line of that summary. It does not follow the job's
  status or link to its card; open Workbench > Delegation to see it.

```text
awaiting_approval -> queued -> worker_running -> review_running -> ready_to_apply -> accepted
                                                     |-> changes_requested       |-> apply_conflict
non-final -> blocked | failed | cancelled
blocked | failed | changes_requested | apply_conflict | queued -> awaiting_approval  (retry, edit, config drift)
blocked | failed -> queued -> review_running  (review again, same worker diff)
```

## How it works

Entry points:

1. **Desktop board.** `Project::open` (`crates/desktop/src/native.rs`) builds
   one `DelegationCoordinator` per open project on the app's Tokio runtime
   (`TokioSpawner`) with `LockRetention::WhileActive`. The Workbench
   **Delegation** tab is `crates/desktop/src/workspace/board.rs`.
   `load_board(true)` runs when the tab opens and on **Refresh**: it calls
   `reconcile` (a held lock makes the board read-only), `list_targets` and
   `list_views`. **New delegation** opens a form for title, objective, scope,
   acceptance checks, a worker and an optional reviewer, and calls
   `create_job` with the active chat as parent. The card list and the
   selected card's detail come from `DelegationJobView`. **Approve and run**,
   **Apply reviewed diff**, **Prepare retry**, **Cancel delegation** and, on a
   blocked or failed card whose saved worker diff can be reviewed again
   (`can_review_again`), **Review again** call `approve`, `apply`, `retry`,
   `cancel` and `review_again` with the displayed `updated_at`,
   on a blocking task, one at a time. The three artifact buttons call
   `artifact_page` with **Previous page**/**Next page**. Coordinator events
   reach the window through `BoardNotifier`, which only bumps a watch channel;
   the board reloads (without reconcile) while the Delegation tab is open. On
   quit, the app calls `shutdown` for a project whose lock it holds. Every
   notification also runs `Workspace::check_delegations` (`workspace/background.rs`),
   which compares card statuses with the last check and toasts a card that moved to
   ready to apply (kept until dismissed), changes requested, blocked, failed or apply
   conflict, even with the board closed. The desktop does not list
   jobs in the sidebar, and cannot edit a card after creating it
   (`update_job`).
2. **`delegate_feature` tool** (`FeatureDelegator`). It has Exec risk and the
   approval target `agent/<id>/feature/<model>`. After the user approves, it
   validates the card and calls `DelegationStore::create` with origin
   `interactive_tool`. It then writes a `DispatchReceipt` bound to the card and
   target fingerprints. On the desktop, the turn's event handler in
   `native.rs` consumes the receipt straight away with
   `apply_dispatch_receipt` when the tool result carries
   `ToolMetadata::Delegation` with a job id. Otherwise the next coordinator
   `reconcile` picks it up. This tool can only target `[agents.<id>]` workers
   with `workspace = "isolated"`.
3. **MCP `delegation_*` tools** on `zest serve`.

Core rules live in `delegation.rs`:

- `FeatureCard::validate` enforces size limits, safe relative paths and
  protected paths.
- `DelegationJob::transition` holds the allowed-transition table. Moving to
  `queued` requires `is_approved()`.
- `approve_with_resolved_targets` records the approval.
- `DelegationStore` writes atomic JSON and migrates v1/v2 records to v3. `save`
  refuses to bring back a job that was cancelled while it ran.
- `ReviewReport::parse` accepts JSON inside code fences, trailing commas and
  decision aliases.
- `validate_diff_scope` checks diff paths against the scope.
- `apply_diff_checked` applies under a process-wide apply lock.

The pipeline is `DelegationCoordinator::run_job` in `scheduler.rs`:

1. `resolve_job_targets` reads `Config::find` again, and the job must still
   match its approval.
2. The job moves to `worker_running`. `FeatureCard::prompt` builds the prompt
   from project docs and context files (at most 24k characters) plus dependency
   summaries.
3. The worker runs:
   - **External**: `run_delegation_worker` in `external_agent.rs` runs a
     headless JSONL CLI, or an ACP JSON-RPC session whose permission requests
     are answered allow and whose fs/terminal calls are confined to the
     worktree. Either way it runs inside `isolated_workspace::prepare`.
   - **Native**: `run_provider_worker` in `delegated_worker.rs` uses
     `RuntimeBuilder` with `RuntimeRole::DelegationWorker`. The worker gets
     read and write tools, no `bash` and no external agents, and approvals are
     bypassed inside the worktree. Providers that own their agent loop are
     rejected.
4. The diff and result artifacts are written.
5. The job moves to `review_running`. `run_acceptance_checks` runs the checks.
6. The reviewer runs through `run_delegation_reviewer` or
   `run_provider_reviewer`. The reviewer role has no write tools.
7. `ReviewReport::parse_verdict` reads the verdict and attaches the recorded
   check results, then `validate_review_paths` runs. `can_accept` chooses
   `ready_to_apply` or `changes_requested`; an accepted verdict over a failed
   or skipped check names that check in the error (`unmet_check`).

`orchestration.rs` keeps an `OrchestrationState` projection inside each job.
It holds the phase log (at most 128 entries), the decision gates (`approval`,
`apply`, `changes`, `recovery`), an inbox (at most 64 messages), the current
dispatch and its heartbeat, the retry state, and external session evidence,
which is not a transcript. The desktop board does not render it; it shows
each attempt's role and agent only.

"Handoff" means two different things here:

- `handoff.rs` `ContextHandoff` is a sanitized JSON view of the parent chat, at
  most 24 KiB. Bearer tokens and secret assignments are redacted, data URLs are
  dropped, and only safe tool-input keys are kept. It is added to the prompt of
  the direct `delegate_external` tool only. Feature cards never get chat
  history.
- `DelegationCoordinator::handoff` returns the worker summary (at most 8000
  characters) and the changed files. The desktop has no **Send to chat**
  action that uses it; the board shows the worker summary on the card.

`delegate_external` (`EXTERNAL_AGENT_TOOL`) is a synchronous run of an
`[agents.<id>]` worker inside a chat turn. It returns the worker's answer and
diff as the tool result and does not apply the diff. It is not a feature card.

When task traces are enabled, native worker/reviewer tasks use the job's
`run-<jobId>` identity. If a chat dispatch tool created the job, its tool trace
joins those tasks to the initiating chat run; scheduling before or after the
dispatch result does not change the association.

`list_targets` lists every `[providers.*]` entry, marked available when
`resolve_provider_target` succeeds, and every `[agents.*]` entry, which must be
isolated. The desktop form lists only available targets, by label, and
**New delegation** is hidden when none is available. The form has no model or
effort override, no **Reconnect** for an unavailable target, and no context or
`dependsOn` fields. There is no automatic fallback. The desktop has no picker
for a CLI worker's own model; set it in that agent's `[agents.<id>]` entry.

## Verify

- `cargo test -p zest-coordinator` runs the full create, approve, review and
  apply cycle against `external_agent_fixture.rs`, compiled with `rustc`.
- `cargo test --locked --release --manifest-path crates/desktop/Cargo.toml --
  offline_workspace_smoke` ends with `workspace/board/selftest.rs`: it builds
  the same fixture worker, creates a card in a GPUI test window, checks that a
  stale approval is refused, approves, waits for the isolated worker and the
  independent review, previews the diff and applies it. `npm run gpui:test` runs
  the same scenario in a real window. Both need `git` and `rustc` on PATH.
- For a manual check, open a Git project with an `[agents.<id>]` worker that
  has `workspace = "isolated"` (`npm run gpui -- --project PATH`), then go to
  Workbench > Delegation.

## Pitfalls

- **Acceptance checks that never pass.** Standard verification commands
  (`cargo check`, `cargo clippy`, `cargo test`, `npm test`, `pytest`, `go test`,
  etc.) auto-run out of the box, whereas unlisted commands such as
  `npm run deploy` or `cargo run` are not auto-run, and a skipped required check
  can never pass. Approval (and the start of each worker run) therefore blocks
  such a card with `acceptance_check_skip_reason`, which names the exact
  `extra_allowlist` entry to add under `[tools.bash]`, or explains that the
  command uses shell syntax or matches the denylist. Checks run as argv
  (`parse_auto_argv`, so quotes group arguments) with the program resolved
  by `resolve_program`, which finds `npm`/`npx`/`pnpm` `.cmd` shims on
  Windows.
- **Reviewer target format.** `ReviewerTarget` is stored adjacently tagged,
  `{"kind":"target","target":{..}}`, the same shape as `ReviewerTargetView`.
  It used to nest one `kind`-tagged enum inside another, writing `kind` twice;
  such a record failed to load, and `DelegationStore::list` used to stop at
  the first unreadable job, so one card with a separate reviewer broke the
  whole board. `list` now skips unreadable job files; `list_all` returns them
  with the reason, and `reconcile` reports each on stderr.
  Its custom `Deserialize` still reads those legacy records (the last `kind`
  wins, which is the target's own tag). Keep a disk round trip in any test of
  record shapes.
- **Diff size limits.** A feature-card worker diff is stored whole, binary
  files included (`git diff --binary`, also for files the worker created). A
  file the worker created over 2 MB (`MAX_NEW_FILE_BYTES`) fails the run with
  its name, and a whole diff over 8 MiB (`MAX_WORKER_DIFF_BYTES`) fails with
  its size, instead of being left out or cut. Only the direct
  `delegate_external` tool still clips its diff at 512 KiB (`clip_diff`),
  because that diff goes to the model, not to `git apply`.
- **No automatic worker retries.** A failed, blocked or rejected job waits
  for a manual `retry`, which needs a new approval. Only the reviewer is
  retried automatically, once. Keep the README saying so.
- **Isolation needs a real repository.** It requires a Git repository with a
  HEAD commit. A folder that only contains repositories gets
  `no_repository_error`. ACP agents cannot use `workspace = "current"`.
- **Unused status.** Nothing assigns the `Planned` status.
