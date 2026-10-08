---
title: Settings, appearance, and profile
summary: The zest.toml config and how Zest edits it, the Settings and Customize panels, themes, fonts, and the profile screen with its avatar.
paths:
  - crates/core/src/config.rs
  - crates/core/src/config_edit.rs
  - crates/core/src/config_migrate.rs
  - crates/core/src/profile.rs
  - zest.toml.example
  - crates/desktop/src/lib.rs
  - crates/desktop/ui/index.html
  - crates/desktop/ui/src/components/SettingsPanel.tsx
  - crates/desktop/ui/src/components/CustomizePanel.tsx
  - crates/desktop/ui/src/components/ThemePicker.tsx
  - crates/desktop/ui/src/components/FontPicker.tsx
  - crates/desktop/ui/src/components/ProfileScreen.tsx
  - crates/desktop/ui/src/components/UserAvatarButton.tsx
  - crates/desktop/ui/src/lib/themes.ts
  - crates/desktop/ui/src/lib/fonts.ts
  - crates/desktop/ui/src/lib/chatView.ts
  - crates/desktop/ui/src/lib/optimizeAvatar.ts
tests:
  - crates/desktop/ui/src/lib/themes.test.ts
  - crates/desktop/ui/src/lib/fonts.test.ts
  - crates/desktop/ui/src/lib/chatView.test.ts
verify:
  - cargo test -p zest-core --lib -- config
  - cargo test -p zest-core --lib -- profile::
  - npm run ui:test
---

# Settings, appearance, and profile

## Behavior
- Config lookup (`Config::find`): `<project>/zest.toml` if present, else `~/.zest/zest.toml`, else a single Anthropic provider from the environment. A project file replaces the user file; the two are never merged.
- `ensure_user_config` writes `~/.zest/zest.toml` from the embedded `zest.toml.example` on first launch and never overwrites an existing file. Credentials never go in `zest.toml`; keys live in the OS credential manager.
- Unknown keys are a parse error (`deny_unknown_fields`). A legacy `kind = "gateway"` provider still loads: `codex` becomes a Codex CLI provider, `claude` a Claude Code provider, others are skipped with a reason. The file on disk is not rewritten.
- `Config::lint` reports non-fatal problems: migration notices first, a default provider that does not exist, ignored legacy `[routing]`, `[tools] max_result_bytes` below 4096, invalid MCP entries, empty commands, and timeouts outside 1..3600 s.
- Desktop edits (providers, MCP servers, external agents) keep comments and formatting, go to the project `zest.toml` when one exists and to `~/.zest/zest.toml` otherwise, and are refused if the result would not parse.
- Customize tabs: Appearance, Typography, Chat view, MCPs, Skills, Extras, Rules, Shortcuts. Appearance, typography, and chat view apply immediately and persist per machine in `localStorage` (`zest.selected_theme`, `zest.selected_font`, `zest.chatViewMode`).
- Themes: `zest` (dark, default), `nights` (dark), `oceanic` (light). An unknown saved id falls back to the default. Fonts: Geist (default), ABC Arizona, Inter, Plus Jakarta Sans, JetBrains Mono, Fira Code, System UI; a font whose CSS fails to load reverts to Geist.
- Settings (Mod+,) is a right-hand dialog with User (name and photo), Response display (stream complete blocks), Provider, CLI delegation, and Usage sections.
- CLI delegation model shortlists include explicit Claude Haiku 5.5 and Fable 5.1; Cursor presets use its confirmed current Opus/Sonnet/Haiku/Fable and Grok 4.7 wire ids. Existing worker configuration is not rewritten.
- The avatar is stored as a JPEG of at most 48,000 bytes; the UI resizes a picked image to 128 px at quality 0.82 first. An empty photo deletes it. Without a photo the avatar shows the name's initial, then a generic icon.
- Profile shows streaks, totals, and a heatmap. Chat counts are retroactive (from thread files in every known workspace); token figures exist only from `meteringSince`, and earlier cells say so rather than showing zero.

## How it works
- Parsing: `Config::parse` (`config.rs`) tries strict TOML first, then `config_migrate::migrate` only if strict parsing failed; on a second failure the original error is shown.
- Editing: `config_edit.rs` (`add_openai_provider`, `add_anthropic_provider`, `add_claude_code_provider`, `add_cursor_provider`, `add_codex_cli_provider`, `add_codex_oauth_provider`, `upsert_external_agent`, `upsert_mcp_server`, `remove_*`) edits a `toml_edit::DocumentMut`, runs `Config::parse` on the result, then `atomic_write`. In `lib.rs`, `editable_config_path` picks the file and `lock_config_edit` serializes edits (a poisoned lock is recovered, not fatal). `open_project_config` opens the project file in the OS editor.
- Theme: `ThemePicker` calls `applyTheme` (`themes.ts`), which sets `data-theme`, the `.dark` class, `color-scheme`, and the color-scheme meta tag, dispatches `zest:theme-changed`, and syncs native chrome (see [app-shell](app-shell.md)). The inline script in `index.html` applies the saved theme before CSS so the first frame matches. Token values live in `index.css` under `[data-theme]`. `App.tsx` reapplies saved font and theme on mount.
- Font: `FontPicker` calls `applyFont` (`fonts.ts`), which sets `--app-font-family` and `data-font`; optional families load lazily through `ensureFontLoaded` (preloaded on hover or focus), and failures are not cached.
- Profile: `ProfileScreen` loads `profileStats`, `usageSnapshot`, and `listSkills` with `allSettled`. The `profile_stats` command gathers `ChatFacts` and ledger days and calls `profile::derive`. `set_local_offset` runs at boot so day boundaries use local time.
- Avatar: `SettingsPanel` uses `optimizeAvatarFile` (`optimizeAvatar.ts`) and then `setUserProfile`. `set_user_profile` in `lib.rs` checks the JPEG magic bytes and size limits and writes `<config dir>/zest/avatar.jpg` and `user-profile.json`.

## Verify
- `cargo test -p zest-core --lib -- config` runs the `config`, `config_edit`, and `config_migrate` tests (comment preservation, gateway migration, lint).
- `cargo test -p zest-core --lib -- profile::` runs the streak and heatmap tests.
- `npm run ui:test` covers the theme, font, and chat-view registries.
- Fixture: open `/?fixture=1`, then Customize > Appearance / Typography; Profile works on canned stats.

## Pitfalls
- `~/.zest/zest.toml` (config) and `<OS config dir>/zest/` (profile, avatar, plugin toggles, last workspace) are different folders.
- A project `zest.toml` is an explicit boundary: the desktop does not borrow the user file's providers for it, which is why a provider can be "not configured for this project".
- The fixture's `setUserProfile` echoes its input, so avatar size limits are enforced only by the real backend.
- The MCP, Skills, Extras, Rules, and Shortcuts tabs render inside `CustomizePanel` but belong to their own features ([plugins](plugins.md), [command-palette-and-shortcuts](command-palette-and-shortcuts.md)).
