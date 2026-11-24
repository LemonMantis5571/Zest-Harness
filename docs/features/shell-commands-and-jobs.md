---
title: Shell commands and background jobs
summary: The agent runs commands with an explicit working directory, read-only ones unattended and everything else after approval, and can keep dev servers running as background jobs it can read and stop.
paths:
  - crates/core/src/tools/bash.rs
  - crates/core/src/tools/capture.rs
  - crates/core/src/tools/jobs.rs
  - crates/core/src/jobs.rs
  - crates/core/src/process_job.rs
  - crates/desktop/src/native.rs
  - crates/desktop/src/workspace.rs
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
- `bash` requires `command`; `cwd` defaults to `.` (the active project) and
  accepts a relative subdirectory or an absolute path for another project.
  A relative `cwd` that escapes the project is refused. An absolute `cwd` outside
  the project is allowed but always needs approval and is shown on the card.
- Risk is always `Exec`. `classify` marks a command `AutoRun` (auto-eligible)
  only if it contains **no shell metacharacters** and starts with an inspection
  or verification prefix: `cargo fmt --check`, `cargo check`,
  `cargo clippy` (no `--fix`), `cargo test`, `cargo tree|metadata|--version`,
  `git status|diff|log|show|branch|rev-parse` (no `-c` / `--exec-path`),
  `npm test`, `npm run lint|test|check|typecheck`, `npx tsc --noEmit`,
  `pytest`, `python -m pytest`, `go test|vet`, `rustc/node --version`, plus
  `[tools.bash] extra_allowlist`. Arbitrary execution (`cargo run`,
  `npm install`, scripts, deploys, mutations) remains gated.
- `[tools.bash] denylist` substrings always force approval, even for allowlisted
  commands. The single exception to the metacharacter rule is the fixed-host
  `curl` GET to `https://x.pcstyle.dev` used by the browse-x skill.
- Auto-run commands are spawned from an argv vector with no shell; approved
  commands run through `cmd /C` (Windows) or `sh -c`.
- Output: stdout+stderr combined, first line `cwd: ...`, then `$ cmd`, `exit N`.
  `bash` keeps up to 256 KiB before middle-clipping (512 KiB per pipe while
  draining), so outputs exceeding `[tools] max_result_bytes` (default 32 KiB)
  spill their full log to `.zest/spill/<thread-id>/` and return a bounded
  preview plus locator instead of losing the middle. A non-zero exit is a
  normal result, not a tool error.
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
- The desktop's Workbench **Jobs** tab lists the active chat's jobs (kind,
  status, label) as of when the tab opened. It cannot read a job's output or
  stop it; the agent's job tools can. A job that ends shows a toast (failures
  stay until dismissed). The desktop does not inject a notice into the owning
  chat; the agent sees the ending only through `job_list` or `job_output`.
- `[tools.bash] enabled = false` removes `bash` but keeps the job tools.

## How it works
1. Registered for the parent chat via `register_exec_tools_with_jobs`
   (tools/mod.rs) from runtime.rs (and foreground-only `Bash` without job tools
   for `RuntimeRole::DelegationWorker` under `ApprovalMode::Auto` +
   `DenyApprover`). The desktop (`Chat::agent` in `crates/desktop/src/native.rs`)
   passes its project's `Project.jobs` registry, one per opened project, with
   `with_job_registry`, and the chat's thread id as owner with `with_job_owner`.
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
6. Desktop: opening the Jobs tab (`Panel::Jobs` in
   `crates/desktop/src/workspace.rs`) calls `JobRegistry::list` with the active
   chat's thread id and renders one line per snapshot. `Workspace::watch_jobs`
   (`workspace/background.rs`) subscribes to `JobEvent`s and toasts each completed job
   of this project: "Job finished: <label>", "Job failed: <label> (<detail>)" (kept
   until dismissed) or "Job stopped: <label>".

## Verify
Unit tests cover `classify`, argv parsing, timeouts, and job ownership.
`tool_path_e2e` is ignored by default because it starts nested Cargo processes;
run it explicitly with the command above.

## Pitfalls
- On Windows, shell commands (and background jobs) run as `cmd /S /C "<command>"` through `raw_arg`. Plain `.arg()` let Rust escape inner quotes as `\"`, which cmd.exe does not understand, so any quoted pattern or path with a space arrived mangled and failed ("cannot find the path specified"). Keep `raw_arg`; `quoted_arguments_reach_cmd_as_typed` covers it.
- The metacharacter check is the whole safety argument for auto-run; any new
  allowlist path must still spawn from argv, never a shell.
- Dropping the timed-out future does not stop the process; the explicit job /
  tree kill does.
- Reading one pipe to EOF before the other deadlocks (`cargo check` writes only
  stderr); keep them drained together.
- `JobRegistry::kill` must not wait on the child mutex (the watcher holds it);
  it kills by PID and lets the watcher publish the terminal state.
- `JobSnapshot.owner_root` is the job's `cwd`, not necessarily the project root;
  do not use it to find a chat's thread store.
- The Jobs tab is a snapshot: it does not refresh while open. Reopen the tab to
  see a status change.
