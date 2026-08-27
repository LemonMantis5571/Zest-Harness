# Wallpaper add-on

This add-on is separate from Zest and is not included in official releases.
The full plugin standard is in
[`docs/PLUGINS.md`](../../../docs/PLUGINS.md).

It copies a chosen image into its plugin folder and renders one of four looks
into a processed file. Zest reads that file from the folder; the add-on never
sends image bytes over the plugin protocol.

| Look     | What it does                                                 |
| -------- | ------------------------------------------------------------ |
| Original | Resized only.                                                |
| Print    | A dotted print texture per colour channel, so it keeps colour. |
| Frosted  | A wide blur, so text layered over it stays readable.         |
| Noir     | Black and white with lifted contrast and a little grain.     |

## What it reads and writes

- Reads the image you pick in the file dialog.
- Writes `state.json`, a `source.*` copy, and `wallpaper.png` or `wallpaper.jpg`
  inside its own plugin folder.
- Does not use the network, credentials, project files, or chat history.

## Build and install

From the workspace root:

```sh
npm run plugin:install -- wallpaper
```

That builds the add-on and copies it to the Zest plugin folder. On Windows:

```text
%LOCALAPPDATA%\Zest\plugins\wallpaper
```

Open Customize > Extras, press Refresh, and turn it on. Choose an image, then
pick a look.

To build without copying files:

```sh
cargo build -p zest-wallpaper-plugin --release
```

To remove it from Zest, turn it off and move the `wallpaper` folder outside
the Zest plugin directory. Zest does not delete plugin files.
