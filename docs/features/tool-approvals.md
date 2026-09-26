---
title: Tool approvals and permission modes
summary: Writes, commands, and other risky tool calls pass one approval gate whose behavior follows the mode picked in the composer (Manual, Accept edits, Plan, Auto, Bypass).
paths:
  - crates/core/src/tools/approval.rs
  - crates/core/src/tools/prepared.rs
  - crates/core/src/agent.rs
  - crates/desktop/ui/src/components/ApprovalModePicker.tsx
  - crates/desktop/ui/src/components/NeedsInputCard.tsx
  - crates/desktop/ui/src/components/ToolCallRow.tsx
  - crates/desktop/ui/src/lib/mcpDisplay.ts
tests:
  - crates/desktop/ui/src/lib/mcpDisplay.test.ts
verify:
  - cargo test -p zest-core --lib -- tools::approval
  - cargo test -p zest-core --lib -- agent::tests
  - cargo test -p zest-desktop --lib -- approval_hub
---

# Tool approvals and permission modes

## Behavior
- Risk comes from the prepared call: `Read` never asks; `Sensitive`, `Write`, and
  `Exec` go through `ApprovalPolicy::decide`.
- Modes (`ApprovalMode`): **Manual** asks for every gated call; **Accept edits**
  applies writes, asks for the rest; **Plan** refuses every gated call with a
  message telling the model to describe the change instead; **Auto** applies
  writes and `auto_eligible` commands (bash read-only allowlist), asks for the
  rest; **Bypass** never asks.
- The desktop starts in Auto (`DESKTOP_DEFAULT_MODE`); core's default is Manual and
  a bare `Agent` uses `DenyApprover`, so an un-wired front-end cannot write.
- The card offers Deny / Allow for session / Allow once. "Allow for session"
  trusts that tool **and the exact target shown** (file path or command line),
  not the whole tool. Plan mode ignores grants.
- Switching mode clears all session grants. Nothing is persisted; restarting the
  app is a clean slate. The mode itself lives in `AppState.policy` and survives
  switching projects.
- Anything that is not an explicit allow denies: unknown decision strings,
  dropped waiters, cancelled turns, poisoned locks.
- Only one card is shown at a time, anchored above the composer
  (`NeedsInputCard`, "1 of N" when several are queued); the transcript shows each
  pending call as a one-line "Awaiting approval" row.
- In Plan mode every sent message is expanded with the `plan` skill (see
  questions-and-planning).

## How it works
1. UI: `ApprovalModePicker` (in Composer, number keys 1-5 pick a mode) calls
   `onApprovalModeChange` in App.tsx, which calls `backend.setApprovalMode` ->
   Tauri `set_approval_mode` (crates/desktop/src/lib.rs) ->
   `ApprovalPolicy::set_mode`. On failure the chip reverts (Rust is authoritative).
   `approval_mode` reads it back.
2. `Agent::execute_tool_calls` prepares every call, runs Read-risk calls
   concurrently, then runs gated calls strictly sequentially via
   `run_gated_call`: `policy.decide(name, preview.path, risk, auto_eligible)` ->
   Allow runs `run_prepared`, Block returns the reason as a tool error, Ask
   continues.
3. On Ask: `approver.prepare(id)` registers the waiter **before**
   `StreamEvent::ApprovalNeeded` is emitted (Sensitive previews lose their diff),
   then `approver.decide` races the cancel token. AllowSession calls
   `policy.trust(name, target)`.
4. Desktop: `HubApprover` / `ApprovalHub` (lib.rs) hold turn-scoped oneshot
   waiters; the UI answers with `resolve_approval(approval_id, "once" | "session"
   | "deny")`, which also records the decision in `ChatPersistence.interrupts`.
   `cancel_turn`, `end_session`, and turn-setup failures call
   `approval_hub.clear()`, which denies every waiter.
5. The UI renders the card with `ToolCallRow asCard`; titles come from
   `approvalTitle` ("Run this command?", "Run manifest on Haiku?").
6. CLI-owned providers (claude_control.rs, codex_app_server.rs, cursor_acp.rs in
   crates/core/src/provider/) map their permission requests onto the same
   `ApprovalPolicy` and `ApprovalNeeded` event.
7. Delegated workers run with `ApprovalMode::Bypass` and `AllowApprover`
   (runtime.rs), which is why bash, browser, ask_user, and MCP tools are withheld
   from them.

## Verify
`tools::approval` covers the mode table; `agent::tests` includes
`gated_tools_run_sequentially_after_the_concurrent_batch` and
`same_file_edits_in_one_batch_are_reprepared_after_the_first_write`. Fixture
mode: open `?fixture=1&scenario=approval` and send a message to get an MCP
approval card; Deny / Allow resolve it.

## Pitfalls
- `auto_eligible` is an input to the mode, never a bypass: Manual still asks and
  Plan still refuses.
- The session-grant target is `preview.path`; if a tool changes what it puts
  there, old grants stop matching (and a vague target would widen them).
- A write prepared early in a batch holds a pre-image; the agent re-prepares it
  if an earlier gated write in the same batch touched the same path.
- Portal-based menus crash the desktop webview; the picker is a plain positioned
  panel on purpose.
