---
title: Settings, appearance, and profile
summary: The zest.toml config and how Zest finds and edits it, the desktop app's Customize pages, and its themes and fonts.
paths:
  - crates/core/src/config.rs
  - crates/core/src/config_edit.rs
  - crates/core/src/config_migrate.rs
  - crates/core/src/profile.rs
  - zest.toml.example
  - crates/desktop/src/theme.rs
  - crates/desktop/src/workspace/chrome.rs
  - crates/desktop/assets/fonts/**
tests:
  - crates/desktop/src/workspace_test.rs
verify:
  - cargo test -p zest-core --lib -- config
  - cargo test -p zest-core --lib -- profile::
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- theme::
  - npm run gpui:test
---

# Settings, appearance, and profile

## Behavior
- Config lookup (`Config::find`): `<project>/zest.toml` if present, else `~/.zest/zest.toml`, else a single Anthropic provider from the environment. A project file replaces the user file; the two are never merged. A worktree chat reads the original project's config (`worktree_registry::config_root`).
- `ensure_user_config` writes `~/.zest/zest.toml` from the embedded `zest.toml.example` on first launch (the desktop app calls it at startup, outside the demo) and never overwrites an existing file. Credentials never go in `zest.toml`; keys live in the OS credential manager.
- Unknown keys are a parse error (`deny_unknown_fields`). A legacy `kind = "gateway"` provider still loads: `codex` becomes a Codex CLI provider, `claude` a Claude Code provider, others are skipped with a reason. The file on disk is not rewritten.
- `Config::lint` reports non-fatal problems: migration notices first, a default provider that does not exist, ignored legacy `[routing]`, `[tools] max_result_bytes` below 4096, invalid MCP entries, empty commands, and timeouts outside 1..3600 s. The desktop app does not display lint results.
- `config_edit.rs` keeps comments and formatting and refuses an edit whose result would not parse. The desktop app has no forms that edit `zest.toml` (providers, MCP servers, external agents); edit the file by hand.
- Customize (sidebar Customize button, or the account row) has the tabs Appearance, Models, Skills & MCP, Add-ons and Usage, each of which belongs to its own feature except Appearance. The page has a "Back to chat" button; Escape also returns to the chat when focus is outside a text field.
- Appearance offers three themes, `zest` (dark, default), `nights` (dark) and `oceanic` (light, marked "Light"), each shown with four color swatches. An unknown saved id falls back to `zest`.
- Fonts: Geist (bundled, default) and System UI (Segoe UI on Windows) always; Inter, Plus Jakarta Sans, JetBrains Mono and Fira Code only when installed on the computer. An unknown saved id falls back to Geist.
- A theme or font choice applies at once to every open window and is saved per machine in `<config dir>/zest/gpui-appearance.json` (`{"theme", "font"}`). Code colors follow the theme's brightness for newly rendered messages; messages already on screen keep theirs until the chat is reopened.
- Oceanic uses a darker destructive color so small warning labels remain legible on its light surfaces.
- The sidebar account row shows a "Z" mark, "Zest" and the current chat's provider, and opens Customize > Models. The desktop app has no user name or photo, no account menu, and no profile screen; `profile.rs` (streaks, totals, heatmap) is currently used by no client.

## How it works
- Parsing: `Config::parse` (`config.rs`) tries strict TOML first, then `config_migrate::migrate` only if strict parsing failed; on a second failure the original error is shown.
- Editing: `config_edit.rs` (`add_openai_provider`, `add_anthropic_provider`, `add_claude_code_provider`, `add_cursor_provider`, `add_codex_cli_provider`, `add_codex_oauth_provider`, `upsert_external_agent`, `upsert_mcp_server`, `remove_*`) edits a `toml_edit::DocumentMut`, runs `Config::parse` on the result, then `atomic_write`.
- Theme: `THEMES` in `theme.rs` holds each palette's color tokens; `set_theme` swaps the active palette and derived colors (`sidebar_line`, `selected_row`, and so on) are mixed from it. `appearance_body` in `workspace/chrome.rs` draws the theme cards and font list; a click calls `set_theme` / `set_face`, then `save_appearance`, which writes the file with `fsutil::atomic_write_json` and refreshes all windows. A save error appears as the workspace note.
- Font: `FACES` lists the offered faces; `available_faces` keeps bundled faces plus families the text system reports as installed. `load_fonts` registers the bundled Geist files from `assets/fonts/`.
- Startup: `theme::application()` calls `load_appearance`, which applies the saved file or the defaults if it is missing or unreadable.

## Verify
- `cargo test -p zest-core --lib -- config` runs the `config`, `config_edit`, and `config_migrate` tests (comment preservation, gateway migration, lint).
- `cargo test -p zest-core --lib -- profile::` runs the streak and heatmap tests.
- `cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- theme::` covers the blended colors, the theme and font fallbacks, and the bundled icons.
- By hand: `npm run gpui -- --demo`, open Customize > Appearance, pick Oceanic and a font, then restart and check the choice is kept.

## Pitfalls
- `~/.zest/zest.toml` (config) and `<OS config dir>/zest/` (appearance, plugin toggles, worktree registry, and the `user-profile.json` / `avatar.jpg` an earlier desktop wrote) are different folders.
- A project `zest.toml` is an explicit boundary: the desktop does not borrow the user file's providers for it, which is why a provider can be "not configured for this project".
- A new theme needs a full `Palette` in `THEMES`; the derived colors are blends, so check contrast on light themes.
- The Models, Skills & MCP, Add-ons, and Usage tabs render under Customize but belong to their own features ([model-and-effort-selection](model-and-effort-selection.md), [skills-and-slash-commands](skills-and-slash-commands.md), [plugins](plugins.md), [usage-and-quota](usage-and-quota.md)).
