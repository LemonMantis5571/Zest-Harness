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
  `worker-result.json` and `review-result.json` in `.zest/delegations/<jobId>/`.
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
- Workers never edit the user's checkout. Each runs in a temporary detached
  `git worktree` that holds the current dirty snapshot (tracked diff plus
  untracked files), with sensitive files removed and `.zest/` left out. What
  comes back is a diff.
- The worker diff must be non-empty, and every path in it must be inside the
  card `scope` (`.` means the whole project) and outside `.git`, `.zest` and
  sensitive paths. Otherwise the job fails.
- Zest runs the acceptance checks itself, in a separate worktree with the worker
  diff applied. It only runs a command that classifies as auto-run under
  `[tools.bash]`. Any other command is recorded as `skipped`. The reviewer must
  report these results exactly as Zest recorded them.
- `ready_to_apply` needs all of the following: a review report that parses,
  decision `accepted`, evidence with `passed` for every required check, and no
  `blocking` finding. A reviewer that edits files sends the job to `blocked`,
  and its edits are discarded.
- Apply checks the scope again and runs `git apply --check` before `git apply`.
  If either fails, the job becomes `apply_conflict` and the checkout is left
  untouched. A successful apply leaves the job `accepted`. Zest saves an
  apply intent before Git touches the checkout. If the coordinator stops
  before recording the outcome, the card becomes `blocked` on reconciliation
  or the next Apply. Inspect the checkout against `worker.diff` before
  requesting a fresh attempt. This also applies to trusted daemons.
- Retry is always manual. `retry` moves a job back to `awaiting_approval`,
  clears its approval and takes a new workspace snapshot. Only one approval
  follows. The next worker attempt also gets the previous diff and review report
  as evidence.
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
   **Apply reviewed diff**, **Prepare retry** and **Cancel delegation** call
   `approve`, `apply`, `retry` and `cancel` with the displayed `updated_at`,
   on a blocking task, one at a time. The three artifact buttons call
   `artifact_page` with **Previous page**/**Next page**. Coordinator events
   reach the window through `BoardNotifier`, which only bumps a watch channel;
   the board reloads (without reconcile) while the Delegation tab is open. On
   quit, the app calls `shutdown` for a project whose lock it holds. The
   desktop does not raise notifications for delegation events, does not list
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
7. `ReviewReport::parse`, `validate_review_paths` and
   `validate_authoritative_checks` run. `can_accept` then chooses
   `ready_to_apply` or `changes_requested`.

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

- **Acceptance checks that never pass.** Commands such as `cargo test` and
  `npm test` are not auto-run, so they are recorded as `skipped`. If the
  reviewer then says `accepted`, the report fails validation with "required
  check lacks evidence" and the job becomes `blocked`. The card can only reach
  `ready_to_apply` if the command prefix is in `[tools.bash] extra_allowlist`.
- **Reviewer target format.** `ReviewerTarget` is stored adjacently tagged,
  `{"kind":"target","target":{..}}`, the same shape as `ReviewerTargetView`.
  It used to nest one `kind`-tagged enum inside another, writing `kind` twice;
  such a record failed to load, and `DelegationStore::list` stops at the first
  unreadable job, so one card with a separate reviewer broke the whole board.
  Its custom `Deserialize` still reads those legacy records (the last `kind`
  wins, which is the target's own tag). Keep a disk round trip in any test of
  record shapes.
- **Clipped external diffs.** External worker diffs are cut in the middle at
  512 KiB (`clip_diff`). A clipped diff will not apply.
- **No automatic retries.** A failed, blocked or rejected job waits for a
  manual `retry`, which needs a new approval. Keep the README saying so.
- **Isolation needs a real repository.** It requires a Git repository with a
  HEAD commit. A folder that only contains repositories gets
  `no_repository_error`. ACP agents cannot use `workspace = "current"`.
- **Unused status.** Nothing assigns the `Planned` status.
