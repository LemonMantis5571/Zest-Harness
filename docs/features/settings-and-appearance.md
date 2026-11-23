---
title: Settings, appearance, and profile
summary: The zest.toml config and how Zest finds and edits it, the desktop app's Customize pages, themes, fonts and chat view, and the profile screen with name and photo.
paths:
  - crates/core/src/config.rs
  - crates/core/src/config_edit.rs
  - crates/core/src/config_migrate.rs
  - crates/core/src/profile.rs
  - zest.toml.example
  - crates/desktop/src/theme.rs
  - crates/desktop/src/workspace/chrome.rs
  - crates/desktop/src/workspace/profile.rs
  - crates/desktop/assets/fonts/**
tests:
  - crates/desktop/src/workspace_test.rs
verify:
  - cargo test -p zest-core --lib -- config
  - cargo test -p zest-core --lib -- profile::
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- theme::
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- workspace::profile::
  - npm run gpui:test
---

# Settings, appearance, and profile

## Behavior
- Config lookup (`Config::find`): `<project>/zest.toml` if present, else `~/.zest/zest.toml`, else a single Anthropic provider from the environment. A project file replaces the user file; the two are never merged. A worktree chat reads the original project's config (`worktree_registry::config_root`).
- `ensure_user_config` writes `~/.zest/zest.toml` from the embedded `zest.toml.example` on first launch (the desktop app calls it at startup, outside the demo) and never overwrites an existing file. Credentials never go in `zest.toml`; keys live in the OS credential manager.
- Unknown keys are a parse error (`deny_unknown_fields`). A legacy `kind = "gateway"` provider still loads: `codex` becomes a Codex CLI provider, `claude` a Claude Code provider, others are skipped with a reason. The file on disk is not rewritten.
- `Config::lint` reports non-fatal problems: migration notices first, a default provider that does not exist, ignored legacy `[routing]`, `[tools] max_result_bytes` below 4096, invalid MCP entries, empty commands, and timeouts outside 1..3600 s. The desktop app does not display lint results.
- `config_edit.rs` keeps comments and formatting and refuses an edit whose result would not parse. The desktop app edits `zest.toml` from Customize > Providers and Customize > MCP servers; external agents are edited by hand.
- Customize (sidebar Customize button) has the tabs Appearance, Chat view, Providers, Models, Advisor, Skills & MCP, MCP servers, Add-ons and Usage. Every tab but Appearance and Chat view belongs to its own feature ([providers-and-sign-in](providers-and-sign-in.md) for Providers, [mcp-servers](mcp-servers.md) for MCP servers). The page has a "Back to chat" button; Escape also returns to the chat when focus is outside a text field.
- Appearance offers three themes, `zest` (dark, default), `nights` (dark) and `oceanic` (light, marked "Light"), each shown with four color swatches. An unknown saved id falls back to `zest`.
- Fonts: Geist (bundled, default) and System UI (Segoe UI on Windows) always; Inter, Plus Jakarta Sans, JetBrains Mono and Fira Code only when installed on the computer. An unknown saved id falls back to Geist.
- A theme or font choice applies at once to every open window and is saved per machine in `<config dir>/zest/gpui-appearance.json` (`{"theme", "font"}`). Code colors follow the theme's brightness for newly rendered messages; messages already on screen keep theirs until the chat is reopened.
- Oceanic uses a darker destructive color so small warning labels remain legible on its light surfaces.
- Chat view: chat history as a Project tree (default) or a Compact list with one mark per chat (needs input, working, pull request, idle), and "Stream complete blocks" (default on), which holds back the Markdown block still being written while a reply streams (see [message-rendering](message-rendering.md)). Both are saved in the same `gpui-appearance.json`.
- The sidebar footer shows the user's photo (or the name's initial, or "Z") and name (or "Zest") and opens Your profile.
- Your profile shows the `profile::derive` stats over the known workspaces and the open project (tokens, requests, busiest day, longest chat, current and longest streak), a 27-week heatmap switchable between Chats and Tokens, an Activity panel, and tokens by provider and by external worker. Token figures exist only from the ledger's start; earlier days say so instead of showing zero. The name saves on Enter; a picked photo is resized to a 128 px JPEG of at most 48,000 bytes, stored as `<config dir>/zest/avatar.jpg` next to `user-profile.json`.
- The app sets the usage day offset from the OS clock at launch, so profile and usage days use local dates.

## How it works
- Parsing: `Config::parse` (`config.rs`) tries strict TOML first, then `config_migrate::migrate` only if strict parsing failed; on a second failure the original error is shown.
- Editing: `config_edit.rs` (`add_openai_provider`, `add_anthropic_provider`, `add_claude_code_provider`, `add_cursor_provider`, `add_codex_cli_provider`, `add_codex_oauth_provider`, `upsert_external_agent`, `upsert_mcp_server`, `remove_*`) edits a `toml_edit::DocumentMut`, runs `Config::parse` on the result, then `atomic_write`.
- Theme: `THEMES` in `theme.rs` holds each palette's color tokens; `set_theme` swaps the active palette and derived colors (`sidebar_line`, `selected_row`, and so on) are mixed from it. `appearance_body` in `workspace/chrome.rs` draws the theme cards and font list; a click calls `set_theme` / `set_face`, then `save_appearance`, which writes the file with `fsutil::atomic_write_json` and refreshes all windows. A save error appears as the workspace note.
- Font: `FACES` lists the offered faces; `available_faces` keeps bundled faces plus families the text system reports as installed. `load_fonts` registers the bundled Geist files from `assets/fonts/`.
- Startup: `theme::application()` calls `load_appearance`, which applies the saved file or the defaults if it is missing or unreadable.
- Profile: `workspace/profile.rs` `load_profile` runs `collect` in the background (`profile::derive` over `profile_roots`, the usage ledger snapshot and the discovered skills); `load_user`, `save_name` and `pick_avatar` read and write `user-profile.json` and `avatar.jpg`, with `avatar_jpeg` stepping the JPEG quality down until the photo fits.

## Verify
- `cargo test -p zest-core --lib -- config` runs the `config`, `config_edit`, and `config_migrate` tests (comment preservation, gateway migration, lint).
- `cargo test -p zest-core --lib -- profile::` runs the streak and heatmap tests; `-- workspace::profile::` on the desktop manifest covers heatmap tones, the grid dates and avatar resizing.
- `cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- theme::` covers the blended colors, the theme and font fallbacks, and the bundled icons.
- By hand: `npm run gpui -- --demo`, open Customize > Appearance, pick Oceanic and a font, then restart and check the choice is kept.

## Pitfalls
- `~/.zest/zest.toml` (config) and `<OS config dir>/zest/` (appearance, profile name and photo, plugin toggles, worktree registry) are different folders.
- A project `zest.toml` is an explicit boundary: the desktop does not borrow the user file's providers for it, which is why a provider can be "not configured for this project".
- A new theme needs a full `Palette` in `THEMES`; the derived colors are blends, so check contrast on light themes.
- The Models, Skills & MCP, MCP servers, Add-ons, and Usage tabs render under Customize but belong to their own features ([model-and-effort-selection](model-and-effort-selection.md), [skills-and-slash-commands](skills-and-slash-commands.md), [mcp-servers](mcp-servers.md), [plugins](plugins.md), [usage-and-quota](usage-and-quota.md)).
