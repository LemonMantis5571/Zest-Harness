---
title: Shell commands and background jobs
summary: The agent runs commands with an explicit working directory, read-only ones unattended and everything else after approval, and can keep dev servers running as background jobs it can read and stop.
paths:
  - crates/core/src/tools/bash.rs
  - crates/core/src/tools/capture.rs
  - crates/core/src/tools/jobs.rs
  - crates/core/src/jobs.rs
  - crates/core/src/process_job.rs
tests:
  - crates/core/tests/tool_path_e2e.rs
verify:
  - cargo test -p zest-core --lib -- tools::bash
  - cargo test -p zest-core --lib -- tools::capture
  - cargo test -p zest-core --lib -- jobs::tests
  - cargo test -p zest-core --test tool_path_e2e -- --ignored --nocapture
---

# Shell commands and background jobs

## Behavior
- `bash` requires `command` and `cwd` (`.` for the project, or an absolute path).
  A relative `cwd` that escapes the project is refused. An absolute `cwd` outside
  the project is allowed but always needs approval and is shown on the card.
- Risk is always `Exec`. `classify` marks a command `AutoRun` (auto-eligible)
  only if it contains **no shell metacharacters** and starts with a read-only
  prefix: `cargo fmt --check`, `cargo tree|metadata|--version`,
  `git status|diff|log|show|branch|rev-parse` (no `-c` / `--exec-path`),
  `npm run lint`, `rustc/node --version`, plus `[tools.bash] extra_allowlist`.
  Builds and tests (`cargo check`, `cargo test`, ...) are deliberately not on it.
- `[tools.bash] denylist` substrings always force approval, even for allowlisted
  commands. The single exception to the metacharacter rule is the fixed-host
  `curl` GET to `https://x.pcstyle.dev` used by the browse-x skill.
- Auto-run commands are spawned from an argv vector with no shell; approved
  commands run through `cmd /C` (Windows) or `sh -c`.
- Output: stdout+stderr combined, first line `cwd: ...`, then `$ cmd`, `exit N`.
  At most 30 KiB is kept, middle clipped with a stated byte count. A non-zero
  exit is a normal result, not a tool error.
- Timeout defaults to 120 s (`[tools.bash] timeout_ms`), capped at 600 s. On
  timeout the whole process tree is killed and the tool errors.
- Provider credentials are scrubbed from the child environment
  (`scrub_secret_environment`) even after approval.
- `background: true` (always needs approval) starts a job, optionally waits for a
  loopback `ready_url` (localhost / 127.0.0.1 / ::1 only; default wait 30 s),
  and returns `server_id: job-N`. Max 8 running background jobs per chat from
  bash (registry cap 32). If the process exits or never becomes ready it is
  killed and its output returned as the error.
- `job_list`, `job_output` (offset, optional wait up to 30 s), and `job_kill`
  (Exec risk) only see jobs owned by the calling chat's thread id.
- When a job finishes, the desktop injects a notice into the owning chat
  (live turn, idle session, or the saved thread on disk) and shows a
  "Background job finished" attention toast.
- `[tools.bash] enabled = false` removes `bash` but keeps the job tools.

## How it works
1. Registered only for the parent chat via `register_exec_tools_with_jobs`
   (tools/mod.rs) from runtime.rs; the desktop passes its process-wide
   `AppState.jobs` registry and the thread id as owner.
2. `Bash::prepare` parses input, runs `classify`, and returns
   `PreparedToolCall::plain_with_preview` with the command line in
   `preview.path` (the approval target) and `auto_eligible` set.
3. `Bash::run` re-classifies, spawns, drains both pipes concurrently with
   `drain_bounded` (capture.rs; head/tail kept while reading, 60 KiB per stream),
   and formats with `render_output` / `clip_middle`.
4. Process trees: Windows starts the child suspended (`windows_creation_flags`),
   assigns it to a kill-on-close job object (`ProcessJob`), then resumes it; Unix
   uses a process group. `terminate_process_tree` uses `taskkill /T /F` or
   `kill -KILL -- -pgid`.
5. `JobRegistry::start_process` (jobs.rs) owns background children, a 256 KiB
   output ring (`next_offset`, `truncated`), status (Running, Stopping,
   Completed, Killed, Failed) and a broadcast of `JobEvent`s.
6. Desktop: `forward_job_events` -> `inject_job_completion` (lib.rs) queues the
   notice and emits `job_completed`; Tauri `list_jobs`, `job_output`, `job_kill`
   expose the registry to the UI, fenced by thread id.

## Verify
Unit tests cover `classify`, argv parsing, timeouts, and job ownership.
`tool_path_e2e` is ignored by default because it starts nested Cargo processes;
run it explicitly with the command above.

## Pitfalls
- The metacharacter check is the whole safety argument for auto-run; any new
  allowlist path must still spawn from argv, never a shell.
- Dropping the timed-out future does not stop the process; the explicit job /
  tree kill does.
- Reading one pipe to EOF before the other deadlocks (`cargo check` writes only
  stderr); keep them drained together.
- `JobRegistry::kill` must not wait on the child mutex (the watcher holds it);
  it kills by PID and lets the watcher publish the terminal state.
- `JobSnapshot.owner_root` is the job's `cwd`, not necessarily the project root,
  so the offline fallback in `inject_job_completion` opens the store there.
- The UI API has `listJobs` / `jobKill`, but no component lists jobs yet.
