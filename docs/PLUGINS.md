# Plugins

Zest plugins are optional add-ons. They are not included in official Zest
releases and are installed separately from the desktop app.

There is no plugin marketplace or automatic download yet. A plugin is a
folder containing a `plugin.json` file and one executable. Zest starts the
executable only after you turn the plugin on.

## Plugin standard v1

This document is the compatibility contract for plugins that Zest may load.
The word **must** means a requirement. A plugin that follows the contract is
Zest-compatible; that does not automatically make it an official Zest plugin.

### What Zest accepts today

The current desktop accepts only plugins that:

- run as a separate child process;
- use manifest protocol `1`;
- contain a valid `plugin.json` and an executable in one plugin folder; and
- implement the JSON request/response contract in this document.

The current desktop accepts `now-playing` and `wallpaper` kinds. The
manifest's `kind` field is reserved for those behavior types. A new kind is
not accepted just because its manifest is valid; it also needs matching host
commands, UI, tests, and a reviewed protocol change.

Zest does not currently accept in-process DLLs, webview code injection,
background services, installers, auto-updaters, or plugins that modify Zest's
own files. A plugin must not need project files, chat history, credentials, or
API keys from Zest because the v1 protocol does not provide them.

### Public plugin acceptance

If Zest later publishes a plugin directory or accepts a community plugin into
this repository, the plugin must also provide:

- public source code and a repeatable build command;
- a compatible license plus dependency notices where required;
- a README with its purpose, supported systems, install/remove steps, and
  known limits;
- a clear list of files, devices, network services, and permissions it uses;
- no hidden downloads, telemetry, credential collection, or code execution;
- safe failure when its target service is missing or unavailable;
- tests for its protocol and important provider/device behavior; and
- a maintainer or issue contact and a version/change note for releases.

Review is required for official distribution. Passing the loader checks alone
is not an endorsement of a plugin's code or safety.

### Compatibility and versioning

- The protocol number is the host/plugin wire contract. Breaking wire changes
  require a new protocol number.
- Additive optional response fields may be added without breaking protocol 1.
- The manifest `version` is the plugin release version and should use SemVer;
  it is separate from `protocol`.
- A plugin ID is permanent. Do not reuse an ID for a different product.
- Unknown manifest keys may be ignored by older Zest versions. Do not depend on
  them for required behavior.
- A plugin that needs a newer protocol must fail with a short explanation
  instead of trying to guess the host behavior.

## Install a plugin

### From the Zest UI

1. Open **Customize > Extras**.
2. Press **Open folder**.
3. Copy one complete plugin folder into the folder that opens.
4. Press **Refresh**.
5. Press **Turn on** for the plugin.

On Windows, the folder is:

```text
%LOCALAPPDATA%\Zest\plugins
```

The folder layout must look like this:

```text
Zest\plugins\
  now-playing\
    plugin.json
    zest-now-playing.exe
  wallpaper\
    plugin.json
    zest-wallpaper.exe
```

Zest checks the folder every time you press **Refresh**. The top-bar button
also appears after the add-on is found; restarting Zest is not required.

### Install an included add-on

From the repository root, with Node and Cargo on PATH:

```sh
npm run plugin:install -- wallpaper
npm run plugin:install -- now-playing
npm run plugin:install -- --all
```

The installer builds the add-on in release mode and copies `plugin.json` plus
the binary into the Zest plugin folder for this OS. Then open **Customize >
Extras**, press **Refresh**, and press **Turn on**.

Now Playing reads the Windows media session, so that add-on is Windows-only.
Wallpaper can be installed on Windows, Linux, or macOS. It starts on the
original image; Print, Frosted, and Noir are opt-in looks.

`npm run dev` starts the desktop app. It does not build or install a plugin.

### Build without installing

```sh
cargo build -p zest-wallpaper-plugin --release
cargo build -p zest-now-playing-plugin --release
```

On Windows the binaries land at `target/release/zest-wallpaper.exe` and
`target/release/zest-now-playing.exe`. On Linux and macOS they are the same
names without `.exe`.

To copy a Now Playing build by hand on Windows:

```powershell
$pluginDir = Join-Path $env:LOCALAPPDATA 'Zest\plugins\now-playing'
New-Item -ItemType Directory -Path $pluginDir -Force | Out-Null
Copy-Item .\target\release\zest-now-playing.exe $pluginDir -Force
Copy-Item .\crates\plugins\now-playing\plugin.json $pluginDir -Force
```

### Remove a plugin from Zest

First press **Turn off**, then move its folder out of the Zest plugin folder.
For example, move `now-playing` somewhere outside
`%LOCALAPPDATA%\Zest\plugins`.

Zest never deletes plugin files. Moving the folder removes it from Zest while
keeping the files available to restore later.

## Build a plugin

The included add-on is a normal Rust workspace package:

```powershell
cargo build -p zest-now-playing-plugin
cargo build -p zest-now-playing-plugin --release
```

For a new Rust add-on, add a package under `crates/plugins/<name>` and add it
to the workspace members in the root `Cargo.toml`. Build it with:

```powershell
cargo build -p <your-package> --release
```

A plugin can also be written in another language. Zest only requires an
executable that follows the JSON protocol below.

## Plugin folder and manifest

Zest installs unpacked folders. A community release should be distributed as
a `.zip` with exactly one top-level plugin folder. Users extract that folder
under `%LOCALAPPDATA%\Zest\plugins`; Zest does not download or unpack archives.

Every plugin must have this shape:

```text
<plugin-id>/
  plugin.json
  <executable>
  README.md       # required for public/community releases
  LICENSE         # required for public/community releases
```

Optional files such as an icon, changelog, or notices file are allowed. The
manifest and executable must be regular files inside the plugin folder. Do not
ship path traversal entries, absolute paths, or symlinks in a release archive.

Example `plugin.json`:

```json
{
  "protocol": 1,
  "id": "now-playing",
  "name": "Now Playing",
  "description": "See and control your music.",
  "version": "0.1.0",
  "executable": "zest-now-playing.exe",
  "kind": "now-playing"
}
```

The machine-readable manifest schema is
[`docs/plugin.schema.json`](plugin.schema.json). The Zest loader performs the
final path and file checks in addition to schema validation.

Manifest rules:

- `protocol` must be `1`.
- `id` must be 1-64 characters, use lowercase ASCII letters, numbers, `.`,
  `_`, or `-`, and match the folder name.
- `name`, `description`, and `version` are display/version values.
- `executable` must be a relative path inside the plugin folder and no longer
  than 260 characters. Absolute paths and `..` are rejected.
- `kind` is optional and defaults to the plugin ID.
- The manifest must be valid UTF-8 JSON and no larger than 32 KiB.

The executable must be present and be a file inside the same plugin folder.
The loader checks the canonical executable path, so a path that escapes the
plugin folder is rejected.

## Protocol version 1

Zest starts a fresh process for every request. It writes one JSON request to
stdin, closes stdin, and waits for one JSON response on stdout. The plugin
must write protocol data only to stdout. Diagnostics sent to stdout will make
the response invalid. The desktop discards stderr, so diagnostics must not be
needed for normal use; use a separate local log only when the plugin really
needs one.

### Requests

Read the current state:

```json
{"action":"get"}
```

Control playback:

```json
{"action":"control","command":"previous"}
{"action":"control","command":"toggle"}
{"action":"control","command":"next"}
```

Set the system volume:

```json
{"action":"setVolume","volumePercent":50}
```

Wallpaper actions (the `wallpaper` kind only):

```json
{"action":"setWallpaper","imagePath":"C:/Users/me/Pictures/bg.jpg","filter":"none"}
{"action":"setWallpaperFilter","filter":"frosted"}
{"action":"clearWallpaper"}
```

`filter` is one of `none`, `print`, `frosted`, or `noir`. Zest normalises the
value before it sends the request, and a plugin should treat anything it does
not recognise as `none` rather than failing.

`get` is shared. A wallpaper plugin returns wallpaper data, not music data.
The processed image is a file in the plugin folder named `wallpaper.png` or
`wallpaper.jpg`. Do not send image bytes in the JSON response.

### Successful response

```json
{
  "ok": true,
  "data": {
    "status": "playing",
    "title": "Example song",
    "artist": "Example artist",
    "album": "Example album",
    "artworkDataUrl": null,
    "sourceApp": "example-player",
    "positionSecs": 12.5,
    "durationSecs": 240,
    "volumePercent": 75,
    "canPrevious": true,
    "canToggle": true,
    "canNext": false,
    "detail": "Ready",
    "observedAt": 1735689600
  },
  "error": null
}
```

`status` should be one of `idle`, `playing`, `paused`, or `stopped`.
Nullable fields may be `null`. `artworkDataUrl`, when present, should be a
data URL such as `data:image/jpeg;base64,...`.

The `canPrevious`, `canToggle`, and `canNext` fields tell Zest which buttons
the current player supports. Use `null` when the capability is unknown; Zest
will keep the button available for older plugins that omit these fields.

A wallpaper `get` / `setWallpaper` success looks like:

```json
{
  "ok": true,
  "data": {
    "status": "ready",
    "sourceName": "bg.jpg",
    "filter": "print",
    "imageFile": "wallpaper.png",
    "detail": "Ready",
    "observedAt": 1735689600
  },
  "error": null
}
```

Wallpaper `status` should be one of `empty` or `ready`. `imageFile` must be a
relative name inside the plugin folder (`wallpaper.png` or `wallpaper.jpg`).
Zest reads that file itself and never asks the plugin to print the image.

### Error response

```json
{
  "ok": false,
  "data": null,
  "error": "The music app is not available."
}
```

The error should be short and safe to show to a user. Zest replaces most
low-level plugin errors with simple UI text.

Protocol limits for v1 are:

- request body: at most 32 KiB;
- response body: at most 512 KiB;
- process time: at most 3 seconds; and
- one request and one response per process.

The process must exit with code `0` after writing a valid response. A timeout,
non-zero exit, invalid JSON, or oversized response is treated as a failed
plugin call.

## Runtime rules

- One process handles one request and then exits.
- Zest gives the process up to 3 seconds to finish.
- Plugin output is limited to 512 KiB.
- Zest starts the process in its plugin folder with stdin and stdout piped.
- Zest clears the inherited environment and restores only the small set of
  Windows runtime variables it needs (`SystemRoot`, `WINDIR`, `TEMP`, `TMP`).
- Zest sends no project files, chat messages, credentials, or API keys through
  the plugin protocol.
- Zest stores only the user's enabled/disabled choice. It does not save the
  plugin response as chat history.

This process boundary keeps plugin code out of the Zest process, but it is not
a security sandbox. A plugin still runs with the user's operating-system
permissions and can access the machine through its own code. Install only
plugins you trust.

The current protocol response for music is `NowPlayingView`. Wallpaper uses
the same envelope with `WallpaperView` data. Optional metadata from Now
Playing may be passed to the current agent turn as untrusted context; titles
and artists are never instructions. Wallpaper is visual only and is not sent
to the agent. A future plugin that contributes agent context must use the
same untrusted, bounded, clearly delimited approach and needs a separate
review.

## Test the sample directly

After building the sample, send it a request without starting Zest:

```powershell
$response = '{"action":"get"}' |
  & .\target\release\zest-now-playing.exe |
  ConvertFrom-Json
$response | Select-Object ok, error
```

The sample's source and manifest are in
[`crates/plugins/now-playing`](../crates/plugins/now-playing). The shared
protocol types are in [`crates/plugin-api`](../crates/plugin-api).

## Troubleshooting

### The plugin is not listed

- Confirm the folder is directly under `Zest\plugins`.
- Confirm it contains `plugin.json`.
- Confirm the folder name matches the manifest `id`.
- Press **Refresh** in **Customize > Extras**.

### It says "Not ready"

The manifest is invalid, the executable is missing, the executable path is
outside the plugin folder, or the `kind` is not supported by this Zest build.
Check the manifest and copy both files again. The current desktop supports
`now-playing` and `wallpaper`.

### The music add-on is listed but shows no song

Turn it on, make sure a Windows media app is playing, and press Refresh. The
sample only reads the active Windows media session; it does not log in to a
music service or fetch music data from the internet.

### Controls do not work

Some players do not expose every media button. The sample reports supported
buttons and Zest disables unavailable ones. If a player changes state outside
Zest, wait briefly for the next refresh.

### The wallpaper did not change

Turn the add-on on, choose an image from Customize > Extras, and wait for the
preview. Large images are resized; the processed file stays in the plugin
folder. The look starts at Original, and changing it re-renders that file.

## Checklist for a new plugin

Before asking for a plugin to be reviewed, confirm:

- [ ] The ID is new, stable, lowercase, and matches the folder name.
- [ ] `plugin.json` uses protocol `1` and a relative executable path.
- [ ] The executable works when launched from its own plugin folder.
- [ ] Every request produces one bounded JSON response and exit code `0`.
- [ ] stdout contains no logs or banners.
- [ ] Missing services and failed commands return useful short errors.
- [ ] The README explains install, removal, platform support, and limits.
- [ ] The source includes tests and a documented build command.
- [ ] The plugin declares what it reads, writes, or sends over the network.
- [ ] No credentials, project content, telemetry, hidden updates, or unsafe
      code execution are added without an explicit future permission design.
- [ ] `cargo fmt`, `cargo clippy`, tests, and a direct protocol smoke test pass
      for Rust plugins.
