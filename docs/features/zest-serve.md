---
title: zest serve (headless coordinator)
summary: A windowless daemon that owns one project's delegation queue and exposes delegation_* tools over authenticated MCP on loopback, so a bot or script can create, approve and apply feature cards.
paths:
  - crates/cli/src/serve.rs
  - crates/cli/src/serve/mcp.rs
tests:
  - crates/cli/tests/serve_e2e.rs
verify:
  - cargo test -p zest --test serve_e2e
  - cargo test -p zest --bin zest serve::mcp
  - cargo test -p zest-coordinator
---

# zest serve (headless coordinator)

The spec is [docs/SERVE.md](../SERVE.md). This daemon hosts the shared
[delegation coordinator](delegation-coordinator.md) and never opens a parent
chat.

## Behavior

- **Starting.** `zest serve --project PATH [--port 0] [--policy gated|trusted] [--init]`.
  - `--project` is required. The path is canonicalized and must be a writable
    directory; this is tested by writing `.zest/.serve-write-probe`.
  - `--init` does three things: it creates a missing directory, runs `git init`
    if needed, and makes an empty `zest init` commit when there is no HEAD. It
    never writes `zest.toml`.
- **Token.** `ZEST_SERVE_TOKEN` is required. It must have at least 32
  characters and no whitespace. The token is read only from the environment and
  never printed.
- **Policy.** The policy is `--policy`, else `ZEST_SERVE_POLICY`, else `gated`.
- **Readiness.** The daemon binds `127.0.0.1` only. When it is ready it prints
  exactly one stdout line:
  `{"kind":"ready","protocol":"zest-serve-v1","pid","projectRoot","mcpUrl","healthUrl","policy"}`.
  It prints that line after the lock is held, after the first `reconcile`, and
  in trusted mode after a first `apply_ready_jobs`. Diagnostics go to stderr.
- **Endpoints.**
  - `GET /healthz` is not authenticated and returns `{"ok":true}`.
  - `POST /mcp` needs `Authorization: Bearer <token>` (otherwise 401). An
    `Origin` header, if present, must be localhost, `127.0.0.1` or `::1`
    (otherwise 403). The body limit is 256 KiB.
- **Methods.** `server/discover`, `initialize` (modern or legacy MCP protocol
  version), `ping`, `tools/list` and `tools/call`. `notifications/*` return
  202.
- **Tools.** `delegation_targets`, `delegation_create`, `delegation_list`,
  `delegation_get`, `delegation_artifact`, `delegation_update`,
  `delegation_approve`, `delegation_retry`, `delegation_cancel` and
  `delegation_apply`. There is no shell tool.
- **Results and errors.** A result is `result.content[0].text`, holding JSON
  text of the job view (or `{targets}`, the list, or an artifact page). Tool
  failures are JSON-RPC errors, not `isError` results:
  - `-32602`: bad arguments;
  - `-32000`: coordinator error;
  - `-32009`: stale `expectedUpdatedAt`;
  - `-32601`: unknown tool or method.
- **`delegation_create`.** It requires `idempotencyKey`, and the origin
  defaults to `inbound_mcp`.
  - `worker` is `{"kind":"provider","providerId","model?","effort?"}` or
    `{"kind":"externalAgent","agentId"}`.
  - `reviewer` is `{"kind":"sameAsWorker"}` or
    `{"kind":"target","target":{...}}`.
- **Gated mode.** Create stays `awaiting_approval` and never writes a dispatch
  receipt. The host calls `delegation_approve`, polls until `ready_to_apply`,
  and then calls `delegation_apply`, which returns `accepted` or
  `apply_conflict`.
- **Trusted mode.** Create also approves. A 2 s scanner calls
  `apply_ready_jobs`, so a passing review ends at `accepted` without any more
  calls. Scope validation and `git apply --check` still run.
- **Shutdown.** Ctrl-C, or SIGTERM on Unix, stops the server, aborts the
  scanner and calls `coordinator.shutdown`. Jobs that were running are saved as
  `blocked`.

## How it works

1. `main.rs` sends `zest serve` to `serve::run`.
2. `serve::run` parses the arguments and calls `require_token` and
   `prepare_project`, including `ensure_git_head` when `--init` is set.
3. It loads `Config::find(root)`: the project `zest.toml` if there is one,
   otherwise `~/.zest/zest.toml`.
4. It builds `DelegationCoordinator::with_runtime` with `TokioSpawner` and
   `NoopNotifier`, then calls `ensure_lock` and `reconcile`.
5. It binds the port, prints the ready line and spawns the 2 s scanner. The
   scanner calls `reconcile`, which ingests receipts written by interactive
   `zest delegate_feature`, marks dead running jobs, and wakes queued jobs. In
   trusted mode it also calls `apply_ready_jobs`.
6. It runs `axum::serve(mcp::router(..))`.

In `mcp.rs`, `mcp_post` runs `authorize` then `check_origin`, then dispatches to
`call_tool`. `call_tool` maps each tool to a coordinator method. `tool_defs`
changes the `delegation_create` description by policy. `tool_content` returns
the serialized result, or, above 256 KiB, a small `response_too_large` JSON
error with `isError: true` so the host always receives valid JSON.

## Verify

- `cargo test -p zest --test serve_e2e` builds the `zest` binary and the
  fixture worker, then checks:
  - a missing token is rejected;
  - the gated create, idempotent create, approve, artifact, apply and repeated
    apply steps;
  - trusted auto-apply;
  - `--init` bootstrapping.
- CI runs this in the `cli-headless` job of `.github/workflows/linux-verify.yml`
  without WebKitGTK.

## Pitfalls

- **Desktop dependency.** `zest` must not depend on `zest-desktop` or WebKit.
  The `cli-headless` job fails if `cargo tree -p zest` mentions either one.
- **Lock sharing with the desktop.** `serve` keeps its project lock for its
  whole run (`LockRetention::Process`). The desktop only holds a project's lock
  while it has delegation work queued or running there, so `serve` starts
  unless the desktop is actively running a job in that project. While `serve`
  owns the project, the desktop board lists its cards read-only.
- **Thread id format.** `parentThreadId` must be `[A-Za-z0-9_-]`, at most 200
  characters (`validate_id`). Host ids containing `:` or `.` are rejected.
- **Oversized responses.** A result over 256 KiB is replaced by a
  `response_too_large` error. `delegation_list` is not paged, so a host with
  many cards should read them one at a time with `delegation_get`.
- **Windows shutdown.** SIGTERM is handled on Unix only; on Windows only Ctrl-C
  triggers a graceful shutdown. A killed daemon's running jobs become `blocked`
  on the next start.
- **Non-Git projects.** Without `--init`, a directory that is not a Git
  repository is accepted, but workers then fail because isolation needs a HEAD
  commit.
