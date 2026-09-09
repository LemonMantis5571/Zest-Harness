# Cursor connection reuse: measured results

Implemented process reuse and explicit conversation session references in the
Rust provider. Requests changing model/effort discard the process. New conversations
get fresh sessions. Missing connections recover context from the local transcript.
Errors and cancellation remove the in-flight connection from the cache.

Two real batches of five requests each completed and the test runners exited
successfully, including process cleanup. Same OceanicUI workspace, prompt and
requested model/effort as the earlier exploratory run. Debug build, no desktop
rendering. Measurements begin at provider entry, not user submission.

| Scenario | Samples | Setup before prompt |
| --- | ---: | --- |
| Previous implementation: fresh process each turn | 5 | 2.839–3.392 s |
| New implementation: reused process, fresh session | 4 | 0.753–1.929 s |
| New implementation: reused process and session | 4 | 0.0325–0.0352 ms |

Each new batch also had one initial process startup: 2.959 s and 4.784 s setup.
There is no demonstrated first-turn improvement. The clear improvement is
skipping repeat initialization and session setup for follow-up turns.

Response totals for reused sessions were 4.052, 2.307, 1.656, and 1.684 seconds.
These repeated questions benefited from conversation history and cannot be
presented as an isolated transport speedup. Fresh-session response totals with
a reused process were 9.636, 7.958, 6.146, and 10.386 seconds. These small,
sequential samples do not establish general response-time or p95 guarantees.

The first batch above predates the parameterized ACP handshake and therefore
recorded `grok-4.6[effort=high,fast=true]` for a
`cursor-grok-4.6-xhigh-fast` request. Follow-up handshake probing showed that
the parameterized response exposes `effort=xhigh` and `fast=true` in ACP
configuration options. Zest now advertises that capability and reconstructs
the requested model identity only after those options verify the selection;
genuine model or effort fallbacks remain visible in provider activity.

Raw data: `reuse-session.jsonl` and `reuse-process-fresh-session.jsonl`.
The report script groups fresh and reused connections separately.

Offline protocol testing verifies same-session continuation, new-conversation
isolation, error cleanup and cancellation cleanup without remote requests.
