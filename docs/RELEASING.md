# Releasing Zest

Zest releases are tag-driven. A `v*` tag starts the release workflow, which
verifies the source on Windows and Linux, builds the native packages and
standalone `zest` CLI binaries, uploads checksums, and publishes a GitHub Release
only after all verification and packaging jobs pass.

A plain SemVer tag such as `v0.3.0` creates a stable release. A tag with a
SemVer prerelease suffix, such as `v0.4.0-rc.1`, creates a prerelease.

## Before tagging

1. Start with a clean worktree. Never commit provider keys, `.env` files,
   signing overlays, or local `zest.toml` files.
2. Set the same version in `Cargo.toml` (`workspace.package.version`) and
   `crates/desktop/tauri.conf.json`.
3. Add release notes at `docs/releases/<version>.md` and move the shipped
   entries from `CHANGELOG.md` into a dated release section.
4. Merge the release preparation through a pull request. Wait for the required
   checks on `master` before creating a tag.
5. Run the full local gate:

   ```powershell
   ./scripts/release-verify.ps1
   ```

   On Linux or macOS, run `bash scripts/release-verify.sh` after installing
   `cargo-audit` and the desktop libraries listed in
   `.github/workflows/linux-verify.yml`.
6. Confirm the version and tag match, then push an annotated tag pointing at
   the verified `master` commit:

   ```powershell
   node scripts/check-release-version.mjs v0.3.0
   git tag -a v0.3.0 -m "release: Zest 0.3.0"
   git push origin v0.3.0
   ```

The gate includes the UI, Rust, generated TypeScript bindings, dependency
audits, and Git whitespace. Live provider checks are not a release gate:
`cargo run -p zest -- doctor --live` consumes real quota and must only be run
with a test account, with the result recorded separately.

The Linux Tauri stack currently brings in GTK3 bindings. `cargo audit` reports
their unmaintained status and the known `glib` unsoundness advisory as
informational transitive warnings; the release gate still fails on blocking
vulnerabilities and does not treat those warnings as a clean bill of health.
Track the Tauri GTK migration before calling the Linux runtime hardened for
hostile content.

## Build and publish

The tagged workflow checks that the tag, Cargo version, and Tauri version
match. It then builds:

- Windows `.msi` and `.exe` installers;
- Linux `.deb`, `.rpm`, and AppImage packages;
- standalone `zest` CLI binaries for Linux and Windows, without WebKit, for
  `zest serve` and the terminal client;
- separate SHA256 manifests for the Windows and Linux installers and CLI
  binaries;
- `LICENSE.txt` and `THIRD_PARTY_NOTICES.md`.

Do not manually upload local build files as a substitute for the tagged
workflow. The workflow publishes the release only after both platform
verification jobs and all package jobs pass.

## Signing and clean-machine checks

Unsigned installers do not prove publisher identity. If signing is enabled
later, keep the private certificate in the certificate store or signing
service and publish only the public certificate fingerprint. The signing
overlay is ignored by Git.

Test the exact uploaded files on a clean Windows profile or Linux machine that
has no Rust, Node.js, source checkout, or old Zest state. Confirm:

- installation and uninstall work;
- no process listens on a loopback port after a restart — the desktop bundles
  and supervises nothing;
- provider setup stores credentials without showing them again;
- a minimal chat can read a file, asks before a write, and handles denial;
- provider status, quota wording, and model selection remain correct;
- free chats and workspace chats appear in the right sidebar sections;
- optional plugins remain opt-in and are not required by the official build;
- the uploaded checksum matches the downloaded installer.

Do not claim live-provider verification from compilation, unit tests, or a
mock server. Never put credentials or response contents in release notes.

## After publication

Install one artifact from the GitHub Release page, not only the local build,
then record its final download URL and checksum in the release record. If an
asset is rebuilt, rerun the workflow from a new tag or update the existing
release with `gh release upload --clobber`; never silently replace a file.
