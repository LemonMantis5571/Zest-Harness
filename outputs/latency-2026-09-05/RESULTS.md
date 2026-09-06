# Real Cursor ACP measurements — 2026-09-05

Five sequential real requests through Zest's Rust CursorAcpProvider, debug
build, with a fresh process and session per turn. Workspace:
`D:\Code\OceanicUI`. Prompt: `what is this project about`. Ask mode,
no MCP servers, no desktop system prompt or rendering. Compilation excluded.
Caches were not cleared; this is fresh-session testing, not cold-cache testing.

Requested model: `cursor-grok-4.6-xhigh-fast`. This historical batch used the
pre-parameterized ACP handshake, so all five responses reported
`grok-4.6[effort=high,fast=true]`. The raw records preserve that baseline; they
do not establish equivalence with Cursor desktop's Extra High.

| Run | Before prompt (s) | First text (s) | Completed (s) |
| --- | ---: | ---: | ---: |
| 1 | 3.174 | 7.663 | 14.178 |
| 2 | 3.392 | 5.330 | 30.462 |
| 3 | 3.270 | 5.566 | 11.294 |
| 4 | 2.839 | 4.849 | 10.388 |
| 5 | 2.879 | 4.806 | 9.164 |

All times start at provider entry. First text may be commentary, not a useful
answer. Five responses returned nonempty content; answer quality was not scored.

Median: 3.174 s preparation, 5.330 s first text, 11.294 s completion.
Process creation alone ranged from 26 to 41 ms. Median initialize RPC was
748 ms; median session/new RPC was 2.238 s. The initialization RPC includes
agent readiness. Phase medians need not sum to the median total.

There is a measurable roughly three-second preparation cost. Persistent
processes might save startup/initialization, but saving session creation also
requires a valid session-reuse design. Neither saving is demonstrated here.
A direct model SDK changes the agent and cannot be assigned this speedup from
these measurements. No UI painting, per-chunk IPC overhead, or Cursor desktop
comparison was measured. Five runs are insufficient for a stable p95 estimate.

Raw records: `overview.jsonl`. Reproduce using the live test documented in
`docs/LATENCY_BENCHMARK.md` and a fresh output filename.

Runner limitation: all five requests returned and flushed their timing records,
but the live test process remained alive during teardown. It was stopped after
collecting the records. This is not a clean full test exit; shutdown requires
separate investigation. The ordinary provider unit suite passed 12 tests.
