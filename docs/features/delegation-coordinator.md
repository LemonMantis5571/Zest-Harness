---
title: Delegation coordinator
summary: The shared queue that owns a project's feature-card jobs: one coordinator per project, two jobs at a time, idempotent approve/cancel/apply, and recovery after a restart.
paths:
  - crates/coordinator/src/lib.rs
  - crates/coordinator/src/lock.rs
  - crates/coordinator/src/runtime.rs
  - crates/coordinator/src/scheduler.rs
  - crates/desktop/src/delegation.rs
tests:
  - crates/core/tests/fixtures/external_agent_fixture.rs
verify:
  - cargo test -p zest-coordinator
  - cargo test -p zest --test serve_e2e
---

# Delegation coordinator

`zest-coordinator` is the scheduler that both the desktop app and `zest serve`
use. The job record and its state machine live in `zest-core`
([delegation](delegation.md)).

## Behavior

- **One coordinator per project.** `CoordinatorLock` holds an exclusive OS lock
  (`flock` on Unix, `LockFileEx` on Windows) on
  `.zest/delegations/coordinator.lock`. Another coordinator on the same root
  gets "another coordinator already owns this project" (`LOCK_HELD_MESSAGE`,
  checked with `is_lock_held_error`). The lock is taken on the first call to any
  coordinator method (`ensure_lock`/`require_lock`).
- **How long the lock is kept** depends on `LockRetention`:
  - `Process` (the default, used by `zest serve`): until the process exits.
  - `WhileActive` (the desktop): only while a job in that project is queued,
    running, or in the `running` map. Each public operation releases an idle
    lock when it returns (`ReleaseWhenIdle`, dropped while the ops mutex is
    still held), and so does the end of a worker task. Opening or listing a
    project in the desktop therefore does not keep `zest serve` out of it.
- **Two jobs at a time.** The limit is `MAX_ACTIVE_WORKER_JOBS = 2`, enforced
  with a semaphore. A job keeps its slot through the worker, the acceptance
  checks and the review.
- **No double runs.** Only a job that is `queued` and `is_approved()` runs. The
  `running` map stops the same job id from being enqueued twice.
- **Dependencies.** A queued job waits until every job in `dependsOn` is
  `accepted`. If a dependency is missing, `failed` or `cancelled`, the job
  becomes `blocked` (`dependency_blocker`). `kick_pending` wakes waiting jobs
  after apply, cancel, fail and reconcile.
- **Serialized, idempotent changes.** Every mutating method takes one ops mutex.
  `approve`, `cancel`, `retry`, `apply` and `update_job` accept
  `expected_updated_at`, and a stale value fails with "changed; expected
  updatedAt". Some repeats return the current view instead of an error, so a
  host can safely retry after losing a response:
  - `approve` on a job that is already approved and past approval;
  - `cancel` on a job that is already cancelled;
  - `apply` on a job that is already accepted.
- **Idempotent create.** `create_job` requires an `idempotency_key` unless the
  origin is `desktop_agent_board` (the default). A repeated key returns the
  existing job. MCP sets the origin to `inbound_mcp`.
- **Heartbeat.** While a worker or reviewer runs, a heartbeat every 15 s updates
  `orchestration.heartbeatAt` and emits `heartbeat`.
- **`reconcile`** does three things:
  - approves jobs that have a valid dispatch receipt (`ingest_inner`);
  - marks `worker_running`/`review_running` jobs with no live task as `blocked`
    ("external delegation process was interrupted");
  - calls `kick_pending`.
- **`shutdown`** stops new enqueues, cancels in-flight tokens and waits up to
  8 s. It then marks jobs that are still running as `blocked`, not `cancelled`.
- **Error classification.** A failed run is marked `blocked` when the error
  mentions "unavailable", "not configured", "owns its agent loop" or "Connect".
  Any other error marks it `failed`.
- **Artifacts.** `artifact_page` serves the three allowed artifacts in 64 KiB
  pages with a `nextOffset`.

## How it works

- `lib.rs` re-exports the public API.
- `runtime.rs` separates spawning from notification:
  - `TaskSpawner`: `TokioSpawner` for serve and tests, and `TauriSpawner` in
    `crates/desktop/src/delegation.rs`.
  - `DelegationNotifier`: `NoopNotifier`, `RecordingNotifier` for tests, and
    the desktop's `TauriDelegationNotifier`, which emits `delegation-event`.
- In `scheduler.rs`, `DelegationCoordinator` holds the lane semaphore, the
  `running` cancel tokens, the ledger, the notifier, the per-root locks and the
  ops mutex.
- **Target resolution.** `resolve_delegation_target` turns a provider target
  into a full target with `resolve_provider_target`. The config fingerprint
  hashes the provider entry. The credential fingerprint hashes the credential
  reference or the `api_key_env` name, never the value. An external agent
  target must be `isolated`.
- **`enqueue`** spawns a task that takes a slot and calls `run_job`. It then
  maps any error through `fail` to `blocked` or `failed`.
- **`job_view`** builds a `DelegationJobView` from the job and its artifacts.
  Changed files come from the diff. Checks show `pending` until a review report
  exists. It also carries the findings and the worker summary.
- **Events.** `DelegationEvent` carries a snake_case `kind` and the full view.
  The kinds are `card_created`, `approval_required`, `queued`,
  `worker_started`, `heartbeat`, `worker_completed`, `reviewer_started`,
  `reviewer_completed`, `changes_requested`, `ready_to_apply`, `applied`,
  `conflict`, `blocked`, `failed` and `cancelled`.

## Verify

`cargo test -p zest-coordinator` covers:

- the full worker, review and apply cycle;
- an MCP-created card staying `awaiting_approval`;
- ingesting a receipt on reconcile;
- a stale revision, and an apply conflict that keeps the file untouched;
- the exclusive lock, and both retention modes (an idle desktop project is
  released; a queued job keeps the lock until its worker finishes);
- restart blocking an interrupted job and resuming queued ones;
- cancel idempotency and dependencies;
- concurrent approve.

These tests need `git` and `rustc` on PATH to build the fixture worker.

## Pitfalls

- **Missing `card_created` on create.** `card_created` is sent by `enqueue`,
  after approval. `create_job` emits no event.
- **Desktop only reconciles on list.** The desktop calls `reconcile` only inside
  `list_delegation_jobs` and never calls `shutdown`. Receipts written by the
  terminal client, and jobs that were running when the app exited, are handled
  the next time the board loads.
- **Read-only board while another process owns the project.** When
  `zest serve` holds the lock, the desktop's `list_delegation_jobs` skips
  `reconcile` and still returns the cards; desktop actions fail with the lock
  message. Any other listing error is still swallowed by `App.tsx`, which shows
  an empty board.
- **Unlock before close.** `CoordinatorLock` drops its OS lock explicitly (`flock(LOCK_UN)` / `UnlockFileEx`) before the file closes. Closing alone releases it only when every copy of the descriptor is gone, and a child another thread is forking (git, a worker) holds one until it execs, so a `WhileActive` release followed by a quick re-acquire failed as "another coordinator". `lock::tests` reproduces it with a cloned handle.
- **Unsure means keep.** Under `WhileActive`, anything that leaves
  `has_active_work` unsure (an unreadable store, a poisoned mutex) keeps the
  lock, on purpose.
- **Config is read at run time.** `Config::find` is read again at approve time
  and at run time. Edits made in between send the job back to
  `awaiting_approval`.
