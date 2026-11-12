---
title: Plugins (Now Playing, Wallpaper)
summary: Optional out-of-process add-ons, off by default, that show and control music or put an image behind the app.
paths:
  - docs/PLUGINS.md
  - docs/plugin.schema.json
  - crates/plugin-api/
  - crates/plugins/
  - crates/desktop/src/plugins.rs
  - crates/desktop/ui/src/components/NowPlayingButton.tsx
  - crates/desktop/ui/src/components/NowPlayingCard.tsx
  - crates/desktop/ui/src/components/WallpaperCard.tsx
  - crates/desktop/ui/src/components/CustomizePanel.tsx
  - crates/desktop/ui/src/lib/pluginSync.ts
  - crates/desktop/ui/src/lib/wallpaperSync.ts
  - crates/desktop/ui/src/lib/nowPlayingCoordinator.ts
  - crates/desktop/ui/src/lib/nowPlayingPluginState.ts
  - scripts/install-plugin.mjs
tests:
  - crates/desktop/ui/src/lib/nowPlayingCoordinator.test.ts
  - crates/desktop/ui/src/lib/nowPlayingPluginState.test.ts
  - crates/desktop/ui/e2e/wallpaper.spec.ts
verify:
  - cargo test -p zest-desktop --lib -- plugins::
  - cargo test -p zest-wallpaper-plugin
  - cargo test -p zest-now-playing-plugin
  - npm run ui:test
  - npm run ui:e2e -- wallpaper
---

# Plugins (Now Playing, Wallpaper)

`docs/PLUGINS.md` is the contract (manifest, protocol v1, limits, review checklist). This page is the map.

## Behavior
- Official builds ship no plugins. A plugin is a folder `<id>/` with `plugin.json` and one executable under the local data dir (`%LOCALAPPDATA%\Zest\plugins` on Windows), found again on every Customize > Extras "Refresh"; no restart is needed.
- Every plugin is off until the user presses "Turn on". Only the on/off choice is stored (`<OS config dir>/zest/plugins.json`). "Open folder" creates and opens the plugins folder; Zest never deletes plugin files.
- Only the kinds `now-playing` and `wallpaper` are supported. An invalid manifest, an id that does not match the folder, protocol other than 1, an executable outside the folder, or an unknown kind lists the plugin as unavailable with a detail such as "Add-on file is not valid." or "This add-on type is not supported.", and it cannot be turned on.
- Each request starts a fresh process in the plugin folder with a cleared environment, one JSON request on stdin (at most 32 KiB), one JSON response on stdout (at most 512 KiB), a 3 s limit, and stderr discarded. A failure shows short generic text, not the plugin's raw output.
- Now Playing (Windows only): a top-bar button appears only when the plugin is installed, available, and on. It shows the track and offers previous/play-pause/next and volume (clamped 0 to 100). Metadata is polled every 5 s only while enabled.
- While Now Playing is on and a track is playing or paused, each turn gets an `<zest-plugin id="now-playing" trust="untrusted-metadata">` block (title, artist, album; one line each, `<` and `>` replaced, 240 characters max) in the volatile part of the system prompt. It is not saved in the transcript.
- Wallpaper (all OSes): choose an image, then a look (`none`, `print`, `frosted`, `noir`, `sepia`, `warm`, `cool`, `muted`). A new image keeps the current look, unknown looks become `none`, and the processed image (at most 6 MiB, PNG or JPEG) is drawn behind the app with `html.has-wallpaper` and `data-wallpaper-filter`. The wallpaper is never sent to the agent.

## How it works
1. Wire contract: `crates/plugin-api/src/lib.rs` (`PROTOCOL_VERSION`, `PluginManifest`, `PluginRequest` = get/control/setVolume/setWallpaper/setWallpaperFilter/clearWallpaper, `NowPlayingView`, `WallpaperView`, `PluginResponse`, `wallpaper_filter`).
2. Host: `crates/desktop/src/plugins.rs`. `discover_plugins` and `load_plugin` validate manifests and canonical paths, `invoke` spawns and bounds the process, and `to_ui` reads `wallpaper.png`/`wallpaper.jpg` from the plugin folder into a data URL. The Tauri commands in `lib.rs` (`list_plugins`, `open_plugins_folder`, `set_plugin_enabled`, `now_playing`, `control_now_playing`, `set_now_playing_volume`, `wallpaper`, `pick_wallpaper` with an `rfd` file dialog, `set_wallpaper_filter`, `clear_wallpaper`) run blocking work on `spawn_blocking`. `turn.rs` calls `plugins::agent_context` once per turn.
3. Samples: `crates/plugins/now-playing/src/main.rs` (Windows media session and endpoint volume; other OSes return "only works on Windows") and `crates/plugins/wallpaper/src/{lib,main}.rs` (copies the source, renders looks with the `image` crate, caps output at 1600 px and 1.44 MP, keeps `state.json`).
4. UI: `CustomizePanel` `PluginsPanel` lists plugins and hosts `NowPlayingCard` and `WallpaperCard`, then broadcasts `zest:plugins-changed` (`pluginSync.ts`) and `zest:wallpaper-changed` (`wallpaperSync.ts`). `App.tsx` reloads the wallpaper on that event. `NowPlayingButton` (in a `TopbarPanel`) derives visibility with `nowPlayingPluginState`/`nowPlayingButtonVisible` and serializes reads and actions with `createNowPlayingCoordinator`, so a slow poll cannot undo a newer click.
5. Install: `npm run plugin:install -- <id>|--all` (`scripts/install-plugin.mjs`) runs `cargo build -p <package> --release`, copies the binary and `plugin.json` (with `executable` rewritten for the OS) into the plugin folder, and skips Now Playing off Windows under `--all`.

## Verify
- `cargo test -p zest-desktop --lib -- plugins::` covers manifest checks, path safety, spawn retry, bounded output, filter normalisation, and the untrusted context block. The wallpaper and now-playing crates have their own tests.
- `npm run ui:e2e -- wallpaper`: fixture Customize > Extras, turn on, choose an image, switch to Sepia.
- By hand: `'{"action":"get"}' | target/release/zest-now-playing.exe`.

## Pitfalls
- A new plugin kind needs host commands, UI, tests, and a protocol review; `supported_kind` is the gate.
- `invoke` clears the child environment and restores only `SystemRoot`, `WINDIR`, `TEMP`, `TMP`, `PATH`, `TMPDIR`, and `LD_LIBRARY_PATH`. Keep that list and `PLUGINS.md` in step.
- Plugin stdout must hold only the response; any logging there makes the response invalid.
- The `zest-plugin-api` crate has no tests of its own.
