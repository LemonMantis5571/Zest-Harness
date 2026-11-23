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
| tokio | MIT | [Tokio](https://github.com/tokio-rs/tokio) |
| toml, toml_edit | MIT or Apache-2.0 | [toml-rs](https://github.com/toml-rs/toml) |

## Desktop app direct dependencies

The desktop app (`crates/desktop`, its own Cargo workspace) adds these direct
dependencies:

| Component | License | Upstream |
| --- | --- | --- |
| gpui, gpui_platform | Apache-2.0 | [Zed](https://github.com/zed-industries/zed) |
| wry | Apache-2.0 or MIT | [wry](https://github.com/tauri-apps/wry) |
| tao | Apache-2.0 | [tao](https://github.com/tauri-apps/tao) |
| accesskit, image, unicode-segmentation, base64, chrono, url | MIT or Apache-2.0 | [Rust ecosystem repositories](https://crates.io/) |
| syntect, pulldown-cmark | MIT | [syntect](https://github.com/trishume/syntect), [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark) |
| pdf-inspector | MIT | [firecrawl/pdf-inspector](https://github.com/firecrawl/pdf-inspector) |
| embed-resource (build only) | MIT | [embed-resource](https://github.com/nabijaczleweli/rust-embed-resource) |

## Native desktop assets

The desktop app embeds these files from `crates/desktop/assets/`:

| Component | License | Upstream |
| --- | --- | --- |
| Geist Regular, Medium and SemiBold 1.9.2 (`assets/fonts/`, license in `Geist-OFL.txt`) | SIL OFL-1.1 | [vercel/geist-font](https://github.com/vercel/geist-font) |
| Lucide icons from lucide-react 1.48.0 (`assets/icons/`, license in `LICENSE`) | ISC | [Lucide](https://github.com/lucide-icons/lucide) |
| mermaid 12.3.0 (`assets/vendor/mermaid/mermaid.min.js`, license in `LICENSE`) | MIT | [mermaid-js/mermaid](https://github.com/mermaid-js/mermaid) |
| KaTeX 0.19.1 with its fonts (`assets/vendor/katex/`, license in `LICENSE`) | MIT | [KaTeX](https://github.com/KaTeX/KaTeX) |

For an exact package-by-package list, inspect the `license` field in each
resolved package under `package-lock.json`/`node_modules` and retain the
corresponding upstream notice when redistributing a binary. Do not remove or
replace upstream copyright and license text with the Zest license.

## Release handling

Release artifacts must include this index and any additional license files
required by the resolved dependency graph. See
[`docs/RELEASING.md`](docs/RELEASING.md) for the release checklist.
