# Now Playing add-on

This add-on is separate from Zest and is not included in official releases.
The full plugin standard is in
[`docs/PLUGINS.md`](../../../docs/PLUGINS.md).

## Build and install

From the workspace root:

```sh
npm run plugin:install -- now-playing
```

That builds the add-on and copies it to the Zest plugin folder. On Windows:

```text
%LOCALAPPDATA%\Zest\plugins\now-playing
```

Open Customize > Extras, press Refresh, and turn it on. Music controls use the
Windows media session, so this add-on is Windows-only.

To build without copying files:

```sh
cargo build -p zest-now-playing-plugin --release
```

To remove it from Zest, turn it off and move the `now-playing` folder outside
the Zest plugin directory. Zest does not delete plugin files.
