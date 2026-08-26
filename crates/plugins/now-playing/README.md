# Now Playing add-on

This add-on is separate from Zest and is not included in official releases.
The full plugin standard is in
[`docs/PLUGINS.md`](../../../docs/PLUGINS.md).

## Build and install

From the workspace root, the easiest install is:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\install-now-playing-plugin.ps1
```

The script builds `zest-now-playing.exe`, copies it with `plugin.json` to:

```text
%LOCALAPPDATA%\Zest\plugins\now-playing
```

Open Settings > Extras, press Refresh, and turn it on.

To build without copying files:

```powershell
cargo build -p zest-now-playing-plugin --release
```

To remove it from Zest, turn it off and move the `now-playing` folder outside
`%LOCALAPPDATA%\Zest\plugins`. Zest does not delete plugin files.
