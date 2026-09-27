---
title: Providers and sign-in
summary: Choose a provider on launch, connect it through a coding CLI sign-in, ChatGPT, or your own API key, and switch providers from inside a chat.
paths:
  - crates/core/src/auth.rs
  - crates/core/src/credentials.rs
  - crates/core/src/codex_oauth.rs
  - crates/core/src/provider/codex_oauth.rs
  - crates/desktop/src/lib.rs
  - crates/desktop/ui/src/components/AuthShell.tsx
  - crates/desktop/ui/src/components/ProviderPicker.tsx
  - crates/desktop/ui/src/components/ApiProviderForm.tsx
  - crates/desktop/ui/src/components/ProviderSwitchSheet.tsx
  - crates/desktop/ui/src/components/WaitingScreen.tsx
  - crates/desktop/ui/src/components/ProviderIcon.tsx
  - crates/desktop/ui/src/lib/loginWait.ts
  - crates/desktop/ui/src/lib/providerVerify.ts
  - crates/desktop/ui/src/lib/chatgptCodex.ts
  - crates/desktop/ui/src/lib/providerMarks.ts
  - crates/desktop/ui/src/lib/providerSelection.ts
tests:
  - crates/desktop/ui/src/lib/loginWait.test.ts
  - crates/desktop/ui/src/lib/providerVerify.test.ts
  - crates/desktop/ui/src/lib/chatgptCodex.test.ts
  - crates/desktop/ui/src/lib/providerMarks.test.ts
  - crates/desktop/ui/src/lib/providerSelection.test.ts
  - crates/desktop/ui/e2e/picker-keyboard.spec.ts
verify:
  - cargo test -p zest-core --lib auth::tests
  - cargo test -p zest-core --lib credentials::tests
  - cargo test -p zest-core --lib codex_oauth
  - cargo test -p zest-desktop --lib characterization
  - npm run ui:test
  - npm run ui:e2e -- picker-keyboard
---

# Providers and sign-in

User flow is described in [GETTING_STARTED.md](../GETTING_STARTED.md). How a
connected provider actually serves turns is in
[provider-runtime](provider-runtime.md) and
[cli-agent-providers](cli-agent-providers.md).

## Behavior

- Launch lists `codex`, `claude`, `cursor` (`PICKER_IDS`), then every other
  `[providers.*]` entry in the active config, plus a `codex-chatgpt` "ChatGPT
  sign-in" row when no `codex_oauth` provider exists and either `codex` is
  configured or the Codex CLI is on PATH. The row sits right after `codex`.
- A row is selectable only when it is both signed in (`AuthStatus` Ready or
  Unknown) **and** configured. Signed in but unconfigured reads "Signed in.
  Configure this provider in Settings." and the picker labels it "Configure".
- Detection never reads or returns secrets: it checks that a credentials file
  exists and parses (`~/.claude/.credentials.json`, `$CODEX_HOME/auth.json`),
  reads only `authInfo.email` from `~/.cursor/cli-config.json`, and treats
  "can't tell" as `Unknown`, never "not logged in".
- Warm launch opens the remembered provider (`last-provider` in the user config
  dir) only if it is ready. A remembered provider that is not ready stops at the
  picker; Zest never silently starts a different one. Launch does not probe.
- Connect runs the vendor login (`claude auth login`, `codex login`) in its own
  console window, or an in-process ChatGPT PKCE sign-in on `localhost:1455`.
  ChatGPT Connect first shows a "Use ChatGPT for Codex" risk confirmation.
- The waiting screen polls every 1.5 s for up to 120 ticks (then "Still
  waiting"), stops after 5 consecutive status failures, and re-checks on window
  focus. A credentials file that was already ready when Reconnect started does
  not count as success.
- After sign-in Zest sends one tiny real turn (`verify_provider`) before opening
  chat. A refused *model* still enters chat with a warning; any other failure
  shows the error and "Back to providers". Only this path says "needs to be
  reconnected", and only for credential failures.
- A failed probe is remembered for 30 minutes (`zest.providerVerify` in
  localStorage): the row shows "Reconnect" and Continue stays disabled.
- API keys are never written to `zest.toml`. Adding a provider writes the entry
  to the project `zest.toml` if one exists, else the user config, and stores the
  key under its `credential` account in the OS credential manager.
- Enable Claude Code / Codex CLI / Cursor CLI writes a parent-provider entry with
  preset models (`sonnet`, `gpt-5.6-sol`, `composer-2.5`) and no API key.
- In-chat switching keeps the thread when both sides run Zest's loop (API key,
  ChatGPT) and forks a copy when a CLI provider is on either side.

## How it works

1. Boot: `App.tsx` boot effect calls `listProviders` + `lastProvider`, then
   `pickReadyProvider` / `pickProviderFallback` (`lib/providerSelection.ts`) and
   `enterChat` → `startSession`. Otherwise it renders `ProviderPicker`.
2. Rows come from `list_providers` in `crates/desktop/src/lib.rs`:
   `detect_all()` (core `auth.rs`) → `provider_view_from_slot`, then
   `append_configured_direct_provider_views` (`configured_provider_view` asks
   `ProviderRegistry::from_config(..).auth_status()`), then
   `append_chatgpt_codex_offer`.
3. Connect: `goConnect` / `reconnectProvider` → `start_login` → core
   `start_claude_code_login`, `start_codex_cli_login` or
   `start_codex_oauth_login` (`codex_oauth::start_login`, callback thread +
   token exchange). Only one login runs at a time (`AppState.login`).
4. Poll: `startWaitingPoll` → `listProviders` + `loginSessionIsNew`
   (`lib/loginWait.ts`) and `login_status` → `LoginProcess::poll_status`. A CLI
   login succeeds when its credentials file is rewritten (`store_rewritten`) or
   the process exits 0. On ChatGPT success `login_status` calls
   `ensure_codex_oauth_configured`, which adds a `codex_oauth` entry (id
   `codex-chatgpt`, or `codex` when free) and keeps reporting `running` until the
   row turns ready.
5. Verify: `finishVerifiedLogin` → `verify_provider` → `probe_provider` →
   core `provider::probe` (max_tokens 1, no thinking). Result goes to
   `markProviderVerified` / `markProviderVerifyFailed` (`lib/providerVerify.ts`).
6. Keys: `ApiProviderForm` → `configure_anthropic_provider` /
   `configure_api_provider` (`config_edit::add_*_provider` +
   `credentials::set`); key replacement → `set_provider_key` (Anthropic and
   OpenAI-compatible entries with a `credential` only).
7. `credentials.rs`: keyring service `zest`, per-process result cache, secrets of
   1280+ UTF-16 units chunked on Windows (`zest-chunked:N` manifest), every write
   read back through a fresh entry; macOS uses an owner-only
   `<data dir>/zest/credentials.json` instead of Keychain.
8. Switch: `ProviderSwitchSheet` (command palette "Switch provider", Settings
   "Change provider") → `switchProvider` → `switch_session_provider`, which uses
   core `thread_provider_handoff` (Stay / InPlace / Copy).
9. ChatGPT turns: `CodexOAuthProvider` (`provider/codex_oauth.rs`) refreshes the
   session via `codex_oauth::refresh_and_store` before each turn, then streams
   through `provider/codex_rig.rs`.

## Verify

- Fixture: `npm run ui:dev`, open `/?fixture=1&scenario=provider-picker`. The
  first `startSession` fails so the picker shows; Continue then opens chat.
  `startLogin` is not available in fixture mode.
- `zest auth` (CLI) prints the same `detect_all()` rows.
- Live sign-in and `verify_provider` spend a real turn; do not automate them.

## Pitfalls

- `start_login` on Windows must not hide the console or null stdio: `claude
  login` then never opens a browser (`LOGIN_CREATION_FLAGS`).
- Cancel must release `localhost:1455` immediately (`CodexOAuthLogin::cancel`
  drops the listener), or the next Connect reports the port in use.
- `credentials::get` caches denials so a keychain prompt is not re-raised by the
  status poll; only `set`/`delete` or a restart clears it.
- `enterChat` marks a verify failure only for `codex` auth errors. Setup errors
  (for example a folder without config) must not turn the row into "Reconnect".
- Being signed in to a vendor CLI is not enough: the provider also needs a config
  entry, or Continue would fail after the click.
- `last-provider` is written only after a session actually started
  (`persist_choice` in `start_session_inner`).
- `backend.configureCodexOAuthProvider` has no UI caller; the OAuth entry is
  written by `login_status`.
