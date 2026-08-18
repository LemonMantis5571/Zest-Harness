# Third-party notices

Zest itself is distributed under the [MIT License](LICENSE). This file indexes
the third-party components used by the source build and desktop bundle. The
bundle ships no third-party executables: every provider is reached either over
HTTP or by spawning a CLI the user installed themselves.

The tables summarize the locked/direct dependency set reviewed for the
2026-08-05 beta preparation. `Cargo.lock` and `package-lock.json` remain the
authoritative dependency inventories; transitive packages keep their upstream
license metadata. Release maintainers must regenerate this index when changing
dependencies and attach any required upstream license texts to the installer
release.

## Rust direct dependencies

| Component | License | Upstream |
| --- | --- | --- |
| anyhow, async-trait, dirs, futures-util, getrandom, keyring, regex, reqwest, serde, serde_json, tempfile, thiserror | MIT or Apache-2.0 | [Rust ecosystem repositories](https://crates.io/) |
| blake3 | CC0-1.0, Apache-2.0, or LLVM exception | [BLAKE3](https://github.com/BLAKE3-team/BLAKE3) |
| dotenvy | MIT | [dotenvy](https://github.com/allan2/dotenvy) |
| globset, ignore | Unlicense or MIT | [ripgrep crates](https://github.com/BurntSushi/ripgrep/tree/master/crates) |
| similar | Apache-2.0 | [similar](https://github.com/mitsuhiko/similar) |
| tauri, tauri-plugin-notification | Apache-2.0 or MIT | [Tauri](https://github.com/tauri-apps/tauri), [Tauri plugins](https://github.com/tauri-apps/plugins-workspace) |
| tokio | MIT | [Tokio](https://github.com/tokio-rs/tokio) |
| toml, toml_edit | MIT or Apache-2.0 | [toml-rs](https://github.com/toml-rs/toml) |

## JavaScript direct dependencies

The desktop webview uses the following direct packages. All are permissive
licenses except the Geist font package, which is distributed under the
SIL Open Font License 1.1.

| Components | License |
| --- | --- |
| React, React DOM, React Markdown, Remark GFM, Mermaid, Shiki, shadcn, Tailwind Merge, tw-animate-css, clsx, lucide-react | MIT or ISC (see each package metadata) |
| Base UI, @shadcn/react, class-variance-authority, @tauri-apps/api, @tauri-apps/plugin-notification | MIT or Apache-2.0 |
| @fontsource-variable/geist | SIL OFL-1.1 |
| Vite, Tailwind CSS, Oxlint, TypeScript, and their plugins/types | MIT or Apache-2.0 |
| Vendored monochrome provider paths from `@lobehub/icons` 5.15.0 ([lobe-icons](https://github.com/lobehub/lobe-icons)) | MIT |

For an exact package-by-package list, inspect the `license` field in each
resolved package under `package-lock.json`/`node_modules` and retain the
corresponding upstream notice when redistributing a binary. Do not remove or
replace upstream copyright and license text with the Zest license.

## Release handling

Release artifacts must include this index and any additional license files
required by the resolved dependency graph. See
[`docs/RELEASING.md`](docs/RELEASING.md) for the release checklist.
