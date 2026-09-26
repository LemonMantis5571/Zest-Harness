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
  - crates/desktop/src/delegation.rs
  - crates/desktop/ui/src/components/WorkbenchPanel.tsx
  - crates/desktop/ui/src/components/OrchestrationStatus.tsx
  - crates/desktop/ui/src/components/WorkerModelPicker.tsx
  - crates/desktop/ui/src/lib/generated/Delegation*.ts
tests:
  - crates/core/tests/fixtures/external_agent_fixture.rs
  - crates/desktop/ui/src/lib/fixtureBackend.test.ts
verify:
  - cargo test -p zest-core --lib delegation
  - cargo test -p zest-core --lib delegated_worker
  - cargo test -p zest-core --lib orchestration
  - cargo test -p zest-core --lib handoff
  - cargo test -p zest-core --lib tools::external_agent
  - cargo test -p zest-core --lib tools::isolated_workspace
  - cargo test -p zest-coordinator
  - npm run test -w ui
  - npm run test:e2e -w ui -- workbench
---

# Delegation (feature cards)

Queueing, locking and restart recovery are in
[delegation-coordinator](delegation-coordinator.md). The headless host is
[zest-serve](zest-serve.md).

## Behavior

- A feature card (title, objective, lane, scope, context, dependsOn,
  acceptanceChecks, worker, reviewer) becomes a job at
  `.zest/delegations/<jobId>.json`. Its artifacts are `worker.diff`,
  `worker-result.json` and `review-result.json` in `.zest/delegations/<jobId>/`.
- Every new card starts in `awaiting_approval`. No worker runs until an approval
  is recorded. That can be the desktop **Approve** button, `delegation_approve`,
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
  untouched. A successful apply leaves the job `accepted`. There is no
  `applied` status.
- Retry is always manual. `retry` moves a job back to `awaiting_approval`,
  clears its approval and takes a new workspace snapshot. Only one approval
  follows. The next worker attempt also gets the previous diff and review report
  as evidence.
- `accepted` and `cancelled` are final. Provider credentials are never written
  to a job. External worker processes run with secret-looking environment
  variables and the configured provider key names removed.

```text
awaiting_approval -> queued -> worker_running -> review_running -> ready_to_apply -> accepted
                                                     |-> changes_requested       |-> apply_conflict
non-final -> blocked | failed | cancelled
blocked | failed | changes_requested | apply_conflict | queued -> awaiting_approval  (retry, edit, config drift)
```

## How it works

Entry points:

1. **Desktop board.** The Workbench **Delegation** tab (`WorkbenchPanel.tsx`)
   calls `lib/api.ts`, which calls the Tauri commands in
   `crates/desktop/src/lib.rs`: `list_delegation_jobs` (runs `reconcile`
   first), `list_delegation_targets`, `create_delegation_job`,
   `update_delegation_job`, `approve_delegation_job`, `cancel_delegation_job`,
   `retry_delegation_job`, `apply_delegation_job`, `get_delegation_job` and
   `prepare_delegation_handoff`. Each one calls `DelegationCoordinator`.
   Coordinator events reach the UI through `TauriDelegationNotifier` (in
   `crates/desktop/src/delegation.rs`) as `delegation-event`, which `App.tsx`
   handles in `handleDelegationEvent`. That handler shows a toast for
   `ready_to_apply`, `changes_requested`, `blocked`, `failed`, `applied` and
   `cancelled`.
2. **`delegate_feature` tool** (`FeatureDelegator`). It has Exec risk and the
   approval target `agent/<id>/feature/<model>`. After the user approves, it
   validates the card and calls `DelegationStore::create` with origin
   `interactive_tool`. It then writes a `DispatchReceipt` bound to the card and
   target fingerprints. On the desktop, `turn.rs` consumes the receipt straight
   away with `apply_dispatch_receipt`. Otherwise the next coordinator
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
which is not a transcript. `OrchestrationStatus.tsx` renders it.

"Handoff" means two different things here:

- `handoff.rs` `ContextHandoff` is a sanitized JSON view of the parent chat, at
  most 24 KiB. Bearer tokens and secret assignments are redacted, data URLs are
  dropped, and only safe tool-input keys are kept. It is added to the prompt of
  the direct `delegate_external` tool only. Feature cards never get chat
  history.
- `DelegationCoordinator::handoff` (the `prepare_delegation_handoff` command)
  returns the worker summary (at most 8000 characters) and the changed files.
  **Send to chat** copies the summary into the composer draft through the
  `zest:delegation-handoff` window event.

`delegate_external` (`EXTERNAL_AGENT_TOOL`) is a synchronous run of an
`[agents.<id>]` worker inside a chat turn. It returns the worker's answer and
diff as the tool result and does not apply the diff. It is not a feature card.

`list_targets` lists every `[providers.*]` entry, marked available when
`resolve_provider_target` succeeds, and every `[agents.*]` entry, which must be
isolated. The create form lets a provider target set a model and an effort. An
unavailable target shows **Reconnect**, and there is no automatic fallback.
`WorkerModelPicker.tsx` (Settings > CLI delegation) sets a CLI worker's own
model through `set_external_agent_model`. It only works for the built-in Claude
Code and Gemini presets.

## Verify

- `cargo test -p zest-coordinator` runs the full create, approve, review and
  apply cycle against `external_agent_fixture.rs`, compiled with `rustc`.
- `npm run test -w ui` includes the "fixture delegation lifecycle" tests in
  `fixtureBackend.test.ts`.
- For a manual check, run `npm run ui:dev` and open `/?fixture=1`, then go to
  Workbench > Delegation.

## Pitfalls

- **Acceptance checks that never pass.** Commands such as `cargo test` and
  `npm test` are not auto-run, so they are recorded as `skipped`. If the
  reviewer then says `accepted`, the report fails validation with "required
  check lacks evidence" and the job becomes `blocked`. The card can only reach
  `ready_to_apply` if the command prefix is in `[tools.bash] extra_allowlist`.
- **Suspected corrupt record with a separate reviewer.** Core `ReviewerTarget`
  and `DelegationTarget` both use serde `tag = "kind"`. A
  `ReviewerTarget::Target(..)` therefore appears to serialize a duplicate
  `kind` key that will not deserialize again. The only test,
  `store_create_preserves_distinct_reviewer_target_on_card_and_job`, never
  reloads the job from disk.
- **Clipped external diffs.** External worker diffs are cut in the middle at
  512 KiB (`clip_diff`). A clipped diff will not apply.
- **No automatic retries.** A failed, blocked or rejected job waits for a
  manual `retry`, which needs a new approval. Keep the README saying so.
- **Isolation needs a real repository.** It requires a Git repository with a
  HEAD commit. A folder that only contains repositories gets
  `no_repository_error`. ACP agents cannot use `workspace = "current"`.
- **Stale TypeScript types.** Regenerate the TS types with
  `cargo test -p zest-desktop --features export-bindings`.
- **Unused status.** Nothing assigns the `Planned` status.
