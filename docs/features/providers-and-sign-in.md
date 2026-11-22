---
title: Providers and sign-in
summary: Configure providers in zest.toml, sign in to a coding CLI or ChatGPT from Customize > Models, and see which provider a chat uses.
paths:
  - crates/core/src/auth.rs
  - crates/core/src/credentials.rs
  - crates/core/src/codex_oauth.rs
  - crates/core/src/provider/codex_oauth.rs
  - crates/desktop/src/workspace.rs
  - crates/desktop/src/native.rs
verify:
  - cargo test -p zest-core --lib auth::tests
  - cargo test -p zest-core --lib credentials::tests
  - cargo test -p zest-core --lib codex_oauth
  - npm run gpui:test
---

# Providers and sign-in

User flow is described in [GETTING_STARTED.md](../GETTING_STARTED.md). How a
connected provider actually serves turns is in
[provider-runtime](provider-runtime.md) and
[cli-agent-providers](cli-agent-providers.md).

## Behavior

- The desktop app has no provider picker on launch. A project window opens
  straight into chat. A chat uses its saved provider (`Thread.provider_id`);
  a new chat uses the config's default target, else the first configured
  provider. The offline demo uses the built-in `demo` provider.
- Providers come only from `[providers.*]` in the active config (see
  [settings-and-appearance](settings-and-appearance.md)). With none
  configured, Customize > Models says "Configure a provider in zest.toml to
  start a turn." and a turn fails with "Choose a configured provider. '<id>'
  is unavailable."
- The desktop app does not read or write the remembered `last-provider`, and
  it does not check sign-in state (`AuthStatus`) before a turn; a missing
  sign-in surfaces as the provider's turn error.
- Customize > Models shows a **Sign in** button for the selected provider when
  it is a `codex_oauth`, `codex_cli` or `claude_code` entry. It runs the vendor
  login (`claude auth login`, `codex login`) in its own console window, or an
  in-process ChatGPT PKCE sign-in on `localhost:1455` stored under the entry's
  `credential` account (else the provider id). There is no Cursor sign-in and
  no ChatGPT risk confirmation.
- While a sign-in runs the workspace note shows the login's browser text and
  the button becomes **Cancel sign-in**, which kills the login ("Sign-in
  cancelled."). The status is polled every 500 ms until it reports "Signed
  in." or the failure detail. Closing the window also kills a running login.
  Only one sign-in runs per window.
- The desktop app does not send a verification turn after sign-in and does not
  add a `codex_oauth` entry to `zest.toml`; configure the entry first.
- The desktop app has no API key form. Add API providers to `zest.toml` by hand
  and store the key in the OS credential manager under the entry's
  `credential` account, or set the entry's `api_key_env` variable. API keys are
  never written to `zest.toml`.
- A worktree chat runs in its isolated checkout but reads provider
  configuration from the original project. This applies to Claude Code, Codex
  CLI, and API providers alike; a missing original project is an error.
  `zest_core::worktree_registry::config_root` resolves that project when the
  desktop opens a project window.
- An existing chat stays on its provider. Picking another provider for it in
  Customize > Models says "Start a new chat to choose a different provider."
  The desktop app has no in-chat provider switch.

## How it works

1. Open: `Project::open_with_registry` (`native.rs`) resolves `config_root`
   and loads `Config::find(config_root)`; `Chat::new` picks the provider;
   `Chat::agent` fails early when the provider id is not in the config.
2. Sign-in: `Workspace::start_login` (`workspace.rs`) calls core
   `auth::start_codex_oauth_login(account)`, `start_codex_cli_login` or
   `start_claude_code_login` and keeps the `LoginProcess` in `Workspace.login`.
   A background loop calls `LoginProcess::poll_status` every 500 ms. A CLI
   login succeeds when its credentials file is rewritten or the process exits
   0. `Drop for Workspace` kills a login still running.
3. `auth::detect_all` checks sign-in state without reading or returning
   secrets: it checks that a credentials file exists and parses
   (`~/.claude/.credentials.json`, `$CODEX_HOME/auth.json`), reads only
   `authInfo.email` from `~/.cursor/cli-config.json`, and treats "can't tell"
   as `Unknown`. `zest auth` (CLI) prints these rows; the desktop app does not
   use them.
4. `credentials.rs`: keyring service `zest`, per-process result cache, secrets of
   1280+ UTF-16 units chunked on Windows (`zest-chunked:N` manifest), every write
   read back through a fresh entry; macOS uses an owner-only
   `<data dir>/zest/credentials.json` instead of Keychain.
5. ChatGPT turns: `CodexOAuthProvider` (`provider/codex_oauth.rs`) refreshes the
   session via `codex_oauth::refresh_and_store` before each turn, then streams
   through `provider/codex_rig.rs`.

## Verify

- `zest auth` (CLI) prints the `detect_all()` rows.
- `npm run gpui:test` renders Customize > Models during the workspace selftest.
- Live sign-in spends a real login; do not automate it.

## Pitfalls

- `start_*_login` on Windows must not hide the console or null stdio: `claude
  login` then never opens a browser (`LOGIN_CREATION_FLAGS`).
- Cancel must release `localhost:1455` immediately (`CodexOAuthLogin::cancel`
  drops the listener), or the next sign-in reports the port in use.
- `credentials::get` caches denials so a keychain prompt is not re-raised by
  later reads; only `set`/`delete` or a restart clears it.
- Being signed in to a vendor CLI is not enough: the provider also needs a config
  entry, or it does not appear in Customize > Models.
- The config is read when the project window opens. Reopen the window after
  editing `zest.toml`.
