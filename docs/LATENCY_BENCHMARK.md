# Measuring Cursor transport latency

Set an absolute output path before launching Zest from the same terminal:

```powershell
$env:ZEST_ACP_TIMING_FILE = "$PWD\cursor-timings.jsonl"
# Launch your usual Zest development or release executable here.
```

The parent directory must already exist. Restart an already-running app to
inherit the variable. Each Cursor ACP provider turn appends one JSON record.
Logging is disabled when the variable is absent; remove it after benchmarking:

```powershell
Remove-Item Env:ZEST_ACP_TIMING_FILE
node scripts/acp-timing-report.mjs cursor-timings.jsonl
```

Use Node 22 or newer for the report. Records contain the requested model and
effort, the served model reported by `session/new` when available, monotonic
elapsed milliseconds, and phase markers, without prompt or response content.
The report groups runs by requested model/effort and served model so server-side
normalization is visible instead of being mistaken for a separate experiment.
Failed or cancelled turns have `completed: false`; the report excludes them from
percentiles and reports their count. A process crash may leave no record. An
unwritable output file does not fail the user's turn.

`process_spawned` measures process creation, not agent readiness. Initialization
can include CLI startup. `session/prompt:sent` marks the start of sending the
prompt. `first_text` is the first nonempty assistant text chunk, which can be
commentary. `completed` means the provider returned a nonempty final response.
All markers start at provider entry, not user submission. These measurements
exclude desktop context preparation and browser painting; they are not an
end-to-end UI benchmark. No token throughput is inferred from character counts.

Run the same tasks repeatedly, alternating application order. Match the exact
model, effort, fast mode, repository state, instructions, and tools. Keep fresh
and warm conversations in separate output files, and record effort/settings
alongside each file. Compare answer correctness and tool work as well as time.
For a user-experience comparison, separately time submission to first useful
answer and completion in each application.

The setup-before-prompt measurement estimates the maximum time recoverable by
eliminating this setup entirely. Process reuse will not necessarily eliminate
session creation, and a direct model SDK changes the agent implementation.
Do not describe that estimate as a measured SDK speedup.

## Run the actual Rust provider without the desktop

This opt-in test makes five real requests using the authenticated Cursor
account and consumes its usage. It requests `cursor-grok-4.6-xhigh-fast`,
uses Ask mode, and reuses its process while creating a fresh session for each
request by default:

```powershell
$env:ZEST_ACP_TIMING_FILE = "$PWD\cursor-timings.jsonl"
$env:ZEST_BENCH_ROOT = 'D:\Code\OceanicUI'
$env:ZEST_BENCH_PROMPT = 'what is this project about'
$env:ZEST_BENCH_REUSE_SESSION = '0'
cargo test -p zest-core live_cursor_latency --lib -- --ignored --nocapture
node scripts/acp-timing-report.mjs cursor-timings.jsonl
```

Use a new output filename for each experiment. The test prints the model
reported by the server; compare that with the requested model rather than
assuming that the provider honored the requested effort. Compilation time is
outside the recorded timings. The desktop's context preparation, system
instructions, and UI are outside this test, so it cannot reproduce a desktop
comparison exactly. Five runs are exploratory, not a reliable tail-latency study.

Set `ZEST_BENCH_REUSE_SESSION=1` to continue the same conversation. The first
turn still starts a process; subsequent turns use the returned session reference.
The report separates fresh processes, reused processes with fresh sessions, and
reused sessions. Repeated questions in one conversation benefit from its history,
so compare setup time independently of total response time.

Production reuse is scoped to a provider instance and an explicit session ID.
Model/effort changes discard the process. Cancellation and errors discard the
in-flight connection; missing/dead connections replay the local transcript in a
fresh session. Provider teardown terminates the owned Windows process tree.
The parameterized ACP handshake exposes the selected effort and fast-mode
options. Zest uses those options to verify a requested model identity before
normalizing the activity label; a genuine model or effort fallback remains
reported as a mismatch.

An offline protocol fixture checks continuation, conversation isolation, failure
and cancellation cleanup (requires Node.js, no account or remote requests):

```powershell
cargo test -p zest-core reused_connection_isolates_sessions_and_discards_failed_turns --lib -- --ignored
```
