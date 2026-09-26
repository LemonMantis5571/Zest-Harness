---
title: Project file tools
summary: The agent reads, searches, lists, creates, and edits files only inside the open project, with likely secrets hidden and oversized results kept retrievable.
paths:
  - crates/core/src/tools/mod.rs
  - crates/core/src/tools/project.rs
  - crates/core/src/tools/sensitive.rs
  - crates/core/src/tools/walk.rs
  - crates/core/src/tools/read_file.rs
  - crates/core/src/tools/write_file.rs
  - crates/core/src/tools/edit_file.rs
  - crates/core/src/tools/glob_files.rs
  - crates/core/src/tools/grep.rs
  - crates/core/src/tools/list_dir.rs
  - crates/core/src/tools/prepared.rs
  - crates/core/src/tools/outcome.rs
  - crates/core/src/tools/spill.rs
  - crates/desktop/ui/src/components/ToolCallRow.tsx
  - crates/desktop/ui/src/components/ToolRunGroup.tsx
  - crates/desktop/ui/src/lib/toolRuns.ts
  - crates/desktop/ui/src/lib/toolDisplay.ts
tests:
  - crates/desktop/ui/src/lib/toolRuns.test.ts
  - crates/desktop/ui/src/lib/toolDisplay.test.ts
verify:
  - cargo test -p zest-core --lib -- tools::characterization
  - cargo test -p zest-core --lib -- tools::project
  - cargo test -p zest-core --lib -- tools::read_file
  - cargo test -p zest-core --lib -- tools::write_file
  - cargo test -p zest-core --lib -- tools::edit_file
  - cargo test -p zest-core --lib -- tools::grep
  - cargo test -p zest-core --lib -- tools::spill
  - npm run ui:test
---

# Project file tools

## Behavior
- Every path is relative to the project root. `ProjectRoot::resolve` canonicalizes
  and refuses anything that lands outside the root (`..`, absolute paths, symlinks
  pointing out). Writes use `resolve_for_write`, which also rejects absolute and
  escaping paths and canonicalizes the nearest existing ancestor.
- Discovery never shows likely secrets: `glob`, directory `grep`, and `list_dir`
  skip anything `is_sensitive_path` matches (`.env`, `.env.*` except
  `.example/.sample/.template/.sample.local`, `*.pem/.key/.p12/.pfx/.keystore`,
  `id_rsa`-style keys, `.netrc`, `credentials.json`, `auth.json`, ...).
- A direct `read_file` or file-scoped `grep` of a sensitive path is
  `ToolRisk::Sensitive`: it asks every time (Bypass excepted), its approval card has
  no diff, its UI summary reads "sensitive content (hidden)", it is redacted from
  durable wire history, and it is never spilled.
- Writing one is gated the same way: `write_file` / `edit_file` on a sensitive
  path prepare as `ToolRisk::Sensitive` (`write_risk` in `sensitive.rs`, applied
  in `PreparedToolCall::write_kind`), so Auto and Accept edits ask instead of
  applying it, and the card keeps its summary but drops the diff. Template env
  files (`.env.example`) stay ordinary writes.
- Walks honor `.gitignore` only in a git work tree or a folder with its own
  `.gitignore`, never read ignore files above the root, do not follow symlinks,
  and always skip `.git`, `.zest`, `target`, `node_modules`.
- Caps are announced, never silent: `read_file` 256 KiB and 2000 lines by default
  (footer says which lines were shown and the next `offset`); `glob` 200 matches;
  `grep` 100 matches, 64 KiB output, 256 KiB per file, 400 chars per line, binary
  files skipped; `list_dir` 500 entries; write/edit bodies 1 MiB.
- `edit_file` only changes an existing UTF-8 file; `old_string` must be non-empty
  and match exactly once unless `replace_all` is true.
- `write_file` / `edit_file` are Write risk. The prepared call binds the path and a
  BLAKE3 pre-image; if the file was created, removed, changed, or re-pointed
  between approval and commit, the write aborts with "fresh approval required".
  Writes are atomic (`fsutil::atomic_write`).
- A tool failure returns to the model as an `is_error` tool result; it never
  aborts the turn.
- Results larger than `[tools] max_result_bytes` (default 32 KiB, `0` = off) are
  stored under `.zest/spill/<thread-id>/` and replaced by a head/tail preview plus
  a locator the model can `read_file` or `grep`. Errors, `read_file` output, and
  Sensitive results are never spilled.
- UI: each call is a compact row (Read, Edit, Find, Search, List, Run...). Two or
  more rows fold into one "Ran 2 commands, edited 1 file, 3 lookups" line with
  +/- counts; later calls join the same group; a call awaiting approval always
  breaks out as its own row. Rows with a diff open the diff viewer.

## How it works
1. `RuntimeBuilder::build` (crates/core/src/runtime.rs) registers
   `register_read_tools` (read_file, list_dir, glob, grep, web_search),
   `register_write_tools` (write_file, edit_file; skipped for the delegation
   reviewer role), and `read_skill` into `worker_tools`, which delegated workers
   clone. Nothing here is registered when a CLI provider owns the agent loop.
2. `Agent::execute_tool_calls` (crates/core/src/agent.rs) calls
   `ToolRegistry::prepare` for each call to get a `PreparedToolCall` (risk,
   `ApprovalPreview`, `PreparedKind::Plain` or `WriteFile`). Read-risk calls run
   concurrently; gated ones run one by one afterwards (see tool-approvals).
3. `ToolRegistry::execute_prepared` dispatches by `tool_name` and applies
   `SpillPolicy::apply`; storage is `SpillStore` (64 files / 64 MiB / 7 days per
   conversation, removed with the thread via `remove_thread_dir`).
4. Both write tools commit through `commit_prepared_write` in write_file.rs;
   diffs come from `bounded_unified_diff` (48 lines / 6000 chars / 8 hunks).
5. Tool events become `tool_call_*` chat events; the UI builds `ToolPart`s,
   `groupToolRuns` / `summarizeTools` (toolRuns.ts) fold them, and
   `ToolRunGroup` / `ToolCallRow` render them.

## Verify
Rust unit tests above; `a_real_grep_spill_can_be_read_back_through_its_locator`
in `tools::characterization` proves the spill loop end to end. UI folding is
covered by toolRuns.test.ts. In `?fixture=1` a sent message shows a tool row;
`&scenario=tool-error` shows a failed row.

## Pitfalls
- The spill locator sits under `.zest/`, which every walk skips; only a direct
  `read_file` or `grep` with that path reaches it.
- `read_file`'s `offset` cannot reach past 256 KiB: the window is applied after
  the byte cap.
- Two edits to one file in one batch share a stale pre-image; the agent
  re-prepares later calls after the first write lands (`should_reprepare`).
- Registration order is part of the cached prompt prefix; reordering tools
  invalidates the prompt cache.
- Keep secret writes on `write_risk`: they were once ordinary Write risk, so Auto
  mode applied an edit to `.env` without asking. The Claude Code and Codex
  bridges use the same helper for their file-change requests.
