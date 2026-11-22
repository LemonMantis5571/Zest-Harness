---
title: Offline demo mode
summary: Run the desktop app with no provider or credentials in a temporary project, with a built-in offline provider that can stream a reply, request a file write, or ask a question.
paths:
  - crates/desktop/src/native.rs
  - crates/desktop/src/workspace.rs
  - crates/desktop/src/workspace/extras.rs
  - crates/desktop/src/workspace/board.rs
tests:
  - crates/desktop/src/workspace_test.rs
  - crates/desktop/src/workspace/board/selftest.rs
verify:
  - npm run gpui:test
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml
---

# Offline demo mode

## Behavior
- `npm run gpui -- --demo` opens the desktop app on a new temporary directory. Nothing is read from or written to a real project, and no provider, API key or sign-in is needed. The directory is deleted when the app exits.
- Every chat uses the built-in `demo` provider with the model `offline`. The provider settings show "Offline demo provider".
- An ordinary message streams a short thinking line and then the reply "Offline reply. This conversation has N wire messages.", word by word, so streaming, stop and history all work offline.
- `Demo: write` makes the provider request `write_file` for `demo.txt`. The request goes through the normal approval card; approving it writes the file in the temporary project, and Changes shows it.
- `Demo: question` makes the provider call `ask_user` with "Which file?" and the choices First and Second, so the question card can be answered.
- Some areas stay off in the demo: Add-ons say they are available in project windows, the Usage panel does not request provider quota, and the delegation board does not list worker targets from configured providers.
- `--demo` skips loading `.env` files and creating `~/.zest/zest.toml`.

## How it works
1. `workspace::launch_if_requested` creates the temporary directory when `--demo` or `--selftest-workspace` is passed and opens it as a project with `demo` set.
2. `native.rs` builds the agent for a demo project from the `Demo` provider, the write tools and the question tool, with the window's approver, questioner and approval policy. Real projects build the agent from `zest.toml` instead.
3. `Demo::stream_turn` checks the last user message for the two trigger phrases and, when tool use is allowed, returns a `tool_use` block; otherwise it streams the canned reply.
4. `--selftest-workspace` also turns the temporary directory into a Git repository (`workspace_test::init_repository`) and drives the window through a scripted session: an approved edit, reopening and reviewing it, and the delegation board's worker, reviewer and apply sequence.

## Verify
- `npm run gpui:test` runs the scripted workspace check in a real window.
- `cargo test --locked --release --manifest-path crates/desktop/Cargo.toml` runs the same scenario in a GPUI test window (Windows CI checks for its `SELFTEST native_workspace: PASS` and `SELFTEST delegation_board: PASS` lines).
- By hand: `npm run gpui -- --demo`, send `Demo: write`, allow the request, and open Changes.

## Pitfalls
- The trigger phrases must be the whole message, exactly `Demo: write` or `Demo: question`.
- Demo chats are saved inside the temporary project, so they are gone after the app exits.
