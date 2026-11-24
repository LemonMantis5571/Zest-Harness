---
title: Tool approvals and permission modes
summary: Writes, commands, and other risky tool calls pass one approval gate whose behavior follows the chat's permission mode (Manual, Accept edits, Plan, Auto, Bypass).
paths:
  - crates/core/src/prefs.rs
  - crates/core/src/tools/approval.rs
  - crates/core/src/tools/prepared.rs
  - crates/core/src/agent.rs
  - crates/desktop/src/interaction.rs
  - crates/desktop/src/native.rs
  - crates/desktop/src/native_view.rs
  - crates/desktop/src/workspace.rs
tests:
  - crates/desktop/src/workspace_test.rs
verify:
  - cargo test -p zest-core --lib -- tools::approval
  - cargo test -p zest-core --lib -- agent::tests
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- offline_workspace_smoke attention_route_follows_window_and_chat_visibility
  - npm run gpui:test
---

# Tool approvals and permission modes

## Behavior
- Risk comes from the prepared call: `Read` never asks; `Sensitive`, `Write`, and
  `Exec` go through `ApprovalPolicy::decide`. Reading or writing a likely secret
  (`.env`, keys) is `Sensitive`, never `Write`, so the modes that auto-apply
  writes still ask for it.
- Modes (`ApprovalMode`): **Manual** asks for every gated call; **Accept edits**
  applies writes, asks for the rest; **Plan** refuses every gated call with a
  message telling the model to describe the change instead; **Auto** applies
  writes and `auto_eligible` commands (bash inspection and verification allowlist), asks for the
  rest; **Bypass** never asks.
- Each desktop chat has its own policy. Real project chats default to **Auto**
  (or the project's saved `approvalMode` in `.zest/session-state.json`), while
  `--demo` chats start in **Manual** so offline approval cards remain testable; a
  bare `Agent` uses `Manual` + `DenyApprover`, so an un-wired front-end cannot
  write. The desktop offers Manual (Ask), Plan, Accept edits, Auto, and Bypass
  under both Customize > Models ("Tool permissions") and the composer status-row
  menu.
- The desktop card offers **Allow once**, **Allow for session** (for non-secret
  writes and commands), and **Deny**. Clicking **Allow for session** returns
  `ApprovalDecision::AllowSession`, which trusts that tool **and the exact
  target shown** (file path for file writes, command tool for CLI providers, or
  exact command string for native `bash`), never a `Sensitive` secret file
  (`.env`, keys). Plan mode ignores grants.
- Switching mode clears all session grants. Choosing a mode in Customize >
  Models saves `approvalMode` in `.zest/session-state.json` as the project's
  default for newly opened chats (session grants themselves remain in-memory
  only).
- `html_preview` uses a fingerprint of its validated title and source as its
  target. Session approval for one document does not authorize changed HTML
  or a different title.
- Anything that is not an explicit allow denies: unknown decision strings,
  dropped waiters, cancelled turns, poisoned locks.
- The desktop shows one card at a time above the composer, oldest first; later
  requests wait behind it with no count shown. The card's title is
  "Allow <tool>? (<risk>)" and its scrollable preview holds the target, the
  summary and the diff. A pending approval in a chat that is not on screen
  raises an "Approval needed" banner, or a system notification while the window
  is not focused.
- In Plan mode every sent message is expanded with the `plan` skill (see
  questions-and-planning).

## How it works
1. Desktop mode: `Chat::new` (`crates/desktop/src/native.rs`) loads
   `ProjectSessionState.approval_mode` (defaulting to `Auto`, or `Manual` in
   `--demo`). The Customize > Models panel (`workspace.rs` `set_approval_mode`)
   calls `ApprovalPolicy::set_mode` on the active chat's `Chat.policy` and
   persists `approvalMode` to `.zest/session-state.json`. `native_view.rs`
   `status_row` reads it back for the status line.
2. `Agent::execute_tool_calls` prepares every call, runs Read-risk calls
   concurrently, then runs gated calls strictly sequentially via
   `run_gated_call`: `policy.decide(name, preview.path, risk, auto_eligible)` ->
   Allow runs `run_prepared`, Block returns the reason as a tool error, Ask
   continues.
3. On Ask: `approver.prepare(id)` registers the waiter **before**
   `StreamEvent::ApprovalNeeded` is emitted (Sensitive previews lose their diff),
   then `approver.decide` races the cancel token. `AllowSession` calls
   `policy.trust(name, target)` when `risk != ToolRisk::Sensitive`.
4. Desktop: `Interactions` (`crates/desktop/src/interaction.rs`) is the chat's
   `Approver`, `Questioner` and `ProviderInteractionHost`. Each request sends an
   `EvKind::Card` holding its own oneshot reply channel to the chat view, which
   queues it in `Root.cards`. `native_view.rs` `native_controls` renders the
   front card; `respond` pops it and sends `Response::Allow` (`AllowOnce`),
   `Response::AllowSession` (`AllowSession`, downgraded to `AllowOnce` if
   `Sensitive`), or `Response::Deny`. A closed event channel or a dropped card
   denies, and the end of a turn clears the queue, which denies every card left.
   `ApprovalNeeded` is recorded in the saved thread with
   `apply_approval_needed`; the live transcript shows no waiting line.
5. CLI-owned providers (claude_control.rs, codex_app_server.rs, cursor_acp.rs in
   crates/core/src/provider/) map their permission requests onto the same
   `ApprovalPolicy` and `ApprovalNeeded` event. On the desktop their command and
   file-change requests arrive as "Allow command?" and "Allow file change?"
   cards.
6. Delegated workers run with `ApprovalMode::Bypass` and `AllowApprover`
   (runtime.rs), which is why bash, browser, ask_user, and MCP tools are withheld
   from them.

## Verify
`tools::approval` covers the mode table; `agent::tests` includes
`gated_tools_run_sequentially_after_the_concurrent_batch` and
`same_file_edits_in_one_batch_are_reprepared_after_the_first_write`. The desktop
`offline_workspace_smoke` test (and `npm run gpui:test`) sends `Demo: write`,
denies the card and checks the file was not written, then allows it and checks the
edit. `attention_route_follows_window_and_chat_visibility` covers when a card
raises a banner or a system notification. Manual check: `npm run gpui -- --demo`,
send `Demo: write`, and answer the card.

## Pitfalls
- `auto_eligible` is an input to the mode, never a bypass: Manual still asks and
  Plan still refuses.
- The session-grant target is `preview.path`; if a tool changes what it puts
  there, old grants stop matching (and a vague target would widen them).
- A write prepared early in a batch holds a pre-image; the agent re-prepares it
  if an earlier gated write in the same batch touched the same path.
- The desktop policy lives on each `Chat`, not on the project: switching chats
  switches modes, and a split pane keeps its own.
