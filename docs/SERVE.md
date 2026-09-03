# `zest serve`

`zest serve` is a windowless coordinator daemon. It owns one project, the
delegation queue, and an inbound MCP endpoint on loopback. It does not open a
parent chat, read prompts from stdin, or load Tauri, WebKit, or dialogs.

This is one of four different things people call “headless” or “MCP” in Zest:

| What | Meaning |
| --- | --- |
| `zest serve` | Persistent coordinator + **inbound** MCP for cards, approval, and apply |
| `zest run --jsonl` | One deny-only parent turn for CI/editor integrations |
| `[agents.<id>].mode = "headless"` | An **external worker CLI** Zest launches for a card |
| `[mcp.<id>]` | **Outbound** MCP servers the parent chat can call |

Any host that can spawn a process and speak MCP can use it: Grokbot, another
bot, a script, an editor plugin. It starts the daemon, waits for the readiness
line, then calls tools.

Default policy is `gated`. Human confirmation stays outside Zest. After that
confirmation the host calls `delegation_approve` and later `delegation_apply`
as separate tools. `--policy trusted` is for a host you already trust with the
bearer token: create starts the worker, and a passing review is applied. Zest
still validates fingerprints, scope, and `git apply --check`. There is no
generic shell tool.

## Spawn

```text
ZEST_SERVE_TOKEN=<at least 32 characters> zest serve --project /path/to/repo --port 0
ZEST_SERVE_TOKEN=<at least 32 characters> zest serve --project /path/to/repo --policy trusted
```

`--project` is required and is the only project this process will serve. The
path is canonicalized and must be a writable directory. `--port 0` (the default)
picks a free loopback port. The daemon binds `127.0.0.1` only.

`ZEST_SERVE_TOKEN` is a high-entropy bearer token. It is never accepted on
argv, never written into the project, and never printed in readiness or logs.

A second coordinator for the same project — desktop or another `zest serve` —
fails on `.zest/delegations/coordinator.lock`.

## Readiness

When the store is open, the lock is held, and the first reconcile has finished,
stdout prints one JSON line. Diagnostics go to stderr.

```json
{"kind":"ready","protocol":"zest-serve-v1","pid":1234,"projectRoot":"/repo","mcpUrl":"http://127.0.0.1:43127/mcp","healthUrl":"http://127.0.0.1:43127/healthz","policy":"gated"}
```

Connect with `Authorization: Bearer $ZEST_SERVE_TOKEN` to `POST /mcp`. `GET
/healthz` is unauthenticated and only reports liveness on loopback. If a
request includes `Origin`, it must be localhost.

## MCP tools

The handshake accepts `server/discover`, `initialize`, `tools/list`, and
`tools/call` using the same protocol versions as Zest's outbound MCP client.

| Tool | Role |
| --- | --- |
| `delegation_targets` | Available worker/reviewer targets |
| `delegation_create` | Create a card. Requires `idempotencyKey`. Stays `awaiting_approval` |
| `delegation_list` / `delegation_get` | Read cards |
| `delegation_artifact` | Paged `worker.diff`, `worker-result.json`, or `review-result.json` |
| `delegation_update` | Edit a card that is still awaiting approval |
| `delegation_approve` | Record human approval, pin fingerprints, enqueue the worker |
| `delegation_retry` | Return a blocked/failed card to `awaiting_approval` |
| `delegation_cancel` | Cancel a non-terminal card |
| `delegation_apply` | Apply a `ready_to_apply` diff after scope + `git apply --check` |

Mutations accept `expectedUpdatedAt`. A stale revision returns a conflict. A
retry of `approve`, `cancel`, or `apply` after a lost HTTP response returns the
state already reached and does not apply a patch twice.

Cards created here record `origin.coordinator = "inbound_mcp"` by default,
plus the `parentThreadId` and `idempotencyKey`. A host may send
`originCoordinator` if it wants a more specific label. Secrets are not stored.

## Policy

`gated` is the default. MCP create stays `awaiting_approval`. The host must
call `delegation_approve` and later `delegation_apply`.

`trusted` is for a bot you already allowed to hold `ZEST_SERVE_TOKEN`. Create
records approval and starts the worker. When review accepts the diff, the
daemon applies it. `delegation_approve` and `delegation_apply` still exist and
stay idempotent. Reviewer rejection still stops at `changes_requested`. Scope
validation and `git apply --check` still run.

Set it with `--policy trusted` or `ZEST_SERVE_POLICY=trusted`. The readiness
line includes `"policy":"trusted"` so the host can tell which daemon it got.

## States and shutdown

In `gated`, a worker does not start until a recorded approval exists: either
`delegation_approve`, or a dispatch receipt written by interactive `zest`
after the tool `y` gate. MCP create never writes that receipt. `trusted`
records that approval on create instead.

SIGINT/SIGTERM stop accepting requests, cancel in-flight workers, and persist
interrupted `worker_running` / `review_running` jobs as `blocked` (not as a
user cancel). Approved `queued` jobs resume after restart. An interrupted
worker or reviewer still needs `retry` and a new approval.

## Interactive Zest

`delegate_feature` in the terminal client writes the card and a dispatch
receipt after `y`. If `zest serve` is already running for that project, the
scanner picks the receipt up. If it is not running yet, the next daemon start
ingests it. The desktop uses the same coordinator and the same lock.
