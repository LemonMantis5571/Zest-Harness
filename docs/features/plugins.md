---
title: Plugins (Now Playing, Wallpaper)
summary: Optional out-of-process add-ons, off by default, that show and control music or put an image behind the app.
paths:
  - crates/desktop/src/workspace/extras.rs
  - docs/PLUGINS.md
  - docs/plugin.schema.json
  - crates/plugin-api/
  - crates/plugins/
  - crates/desktop/src/plugins.rs
  - scripts/install-plugin.mjs
verify:
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- plugins::
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- extras::
  - cargo test -p zest-wallpaper-plugin
  - cargo test -p zest-now-playing-plugin
---

# Plugins (Now Playing, Wallpaper)

`docs/PLUGINS.md` is the contract (manifest, protocol v1, limits, review checklist). This page is the map.

## Behavior
- Official builds ship no plugins. A plugin is a folder `<id>/` with `plugin.json` and one executable under the local data dir (`%LOCALAPPDATA%\Zest\plugins` on Windows). Plugins are listed when a project window opens and again on Customize > Add-ons, its "Refresh add-ons" button, or a toggle; no restart is needed.
- Every plugin is off until the user presses "Turn on". Only the on/off choice is stored (`<OS config dir>/zest/plugins.json`). "Open add-ons folder" creates and opens the plugins folder; Zest never deletes plugin files.
- Only the kinds `now-playing` and `wallpaper` are supported. An invalid manifest, an id that does not match the folder, protocol other than 1, an executable outside the folder, or an unknown kind lists the plugin with a detail such as "Add-on file is not valid." or "This add-on type is not supported.", and it has no Turn on button.
- Each request starts a fresh process in the plugin folder with a cleared environment, one JSON request on stdin (at most 32 KiB), one JSON response on stdout (at most 512 KiB), a 3 s limit, and stderr discarded. A failure shows short generic text as the workspace note, not the plugin's raw output.
- The offline demo shows "Local add-ons are available in project windows." and runs no plugins.
- Now Playing (Windows only): while the plugin is on, a music button labelled with the track title (or "Now Playing") appears in the chat header and opens Customize > Add-ons. The tab shows title, artist and status, Previous / Play / pause / Next (a control the plugin marks unsupported is hidden), and Quieter / Louder in 10-point steps when the plugin reports a volume (clamped 0 to 100 by the host). Metadata is polled every 5 s while it is enabled; polling stops when the plugin reports `disabled`.
- While Now Playing is on and a track is playing or paused, each turn gets an `<zest-plugin id="now-playing" trust="untrusted-metadata">` block (title, artist, album; one line each, `<` and `>` replaced, 240 characters max) under `# Enabled local integrations` in the volatile part of the system prompt. It is not saved in the transcript.
- Wallpaper (all OSes): "Choose image" opens a file dialog; once an image is set, the tab offers the looks `none`, `print`, `frosted`, `noir`, `sepia`, `warm`, `cool`, `muted` and "Clear wallpaper". A new image keeps the current look and unknown looks become `none`. The processed image (at most 6 MiB, PNG or JPEG) is drawn behind the whole window at 25% opacity, cropped to cover. The wallpaper is never sent to the agent.

## How it works
1. Wire contract: `crates/plugin-api/src/lib.rs` (`PROTOCOL_VERSION`, `PluginManifest`, `PluginRequest` = get/control/setVolume/setWallpaper/setWallpaperFilter/clearWallpaper, `NowPlayingView`, `WallpaperView`, `PluginResponse`, `wallpaper_filter`).
2. Host: `crates/desktop/src/plugins.rs`. `discover_plugins` and `load_plugin` validate manifests and canonical paths, `invoke` spawns and bounds the process, and `to_ui` reads `wallpaper.png`/`wallpaper.jpg` from the plugin folder into a data URL. The entry points the window uses are `list`, `set_enabled`, `ensure_plugin_folder`, `now_playing`, `control`, `set_volume`, `wallpaper`, `set_wallpaper`, `set_wallpaper_filter` and `clear_wallpaper`. `Chat::run` (`native.rs`) calls `plugins::agent_context` on a blocking thread once per turn.
3. Window: `workspace/extras.rs`. `Workspace::apply_extra` runs one `Action` (Refresh, Enable, Play, Volume, Choose, Look, Clear) on the background executor, ignores clicks while one is running, and stores the rows, track and wallpaper in `Extras`. `image_from_url` turns the wallpaper data URL (PNG or JPEG only) into a GPUI image, which `Workspace::render` draws behind the sidebar and content. `poll_music` refreshes the track every 5 s. `extras_body` draws the Add-ons tab, and `track_label` feeds the header button in `workspace/chrome.rs`.
4. Samples: `crates/plugins/now-playing/src/main.rs` (Windows media session and endpoint volume; other OSes return "only works on Windows") and `crates/plugins/wallpaper/src/{lib,main}.rs` (copies the source, renders looks with the `image` crate, caps output at 1600 px and 1.44 MP, keeps `state.json`).
5. Install: `npm run plugin:install -- <id>|--all` (`scripts/install-plugin.mjs`) runs `cargo build -p <package> --release`, copies the binary and `plugin.json` (with `executable` rewritten for the OS) into the plugin folder, and skips Now Playing off Windows under `--all`.

## Verify
- `cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- plugins::` covers manifest checks, path safety, spawn retry, bounded output, filter normalisation, and the untrusted context block. `-- extras::` checks that only PNG and JPEG data URLs become a wallpaper. The wallpaper and now-playing crates have their own tests.
- By hand: `npm run plugin:install -- wallpaper`, open a project with `npm run gpui`, then Customize > Add-ons: Turn on, Choose image, switch to Sepia.
- By hand: `'{"action":"get"}' | target/release/zest-now-playing.exe`.

## Pitfalls
- A new plugin kind needs host functions in `plugins.rs`, UI in `extras.rs`, tests, and a protocol review; `supported_kind` is the gate.
- `invoke` clears the child environment and restores only `SystemRoot`, `WINDIR`, `TEMP`, `TMP`, `PATH`, `TMPDIR`, and `LD_LIBRARY_PATH`. Keep that list and `PLUGINS.md` in step.
- Plugin stdout must hold only the response; any logging there makes the response invalid.
- The `zest-plugin-api` crate has no tests of its own.
