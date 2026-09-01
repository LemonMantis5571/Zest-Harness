# Releasing Zest

Zest releases are tag-driven. A `v*` tag starts one workflow that verifies the
source on Windows and Linux, builds the native packages, uploads checksums,
and creates a GitHub prerelease only after both platforms pass.

## Before tagging

1. Start with a clean worktree. Never commit provider keys, `.env` files,
   signing overlays, or local `zest.toml` files.
2. Set the same version in `Cargo.toml` (`workspace.package.version`) and
   `crates/desktop/tauri.conf.json`.
3. Add release notes at `docs/releases/<version>.md`.
4. Review `CHANGELOG.md` and the beta limitations.
5. Run the full gate locally:

   ```powershell
   ./scripts/release-verify.ps1
   ```

   On Linux or macOS, run `bash scripts/release-verify.sh` after installing
   `cargo-audit` and the desktop libraries listed in
   `.github/workflows/linux-verify.yml`.

The gate includes the UI, Rust, generated TypeScript bindings, dependency
audits, and Git whitespace. Live provider checks are not a
release gate: `cargo run -p zest -- doctor --live` consumes real quota and must
only be run with a test account, with the result recorded separately.

The Linux Tauri stack currently brings in GTK3 bindings. `cargo audit` reports
their unmaintained status and the known `glib` unsoundness advisory as
informational transitive warnings; the beta gate still fails on vulnerabilities
but does not treat those warnings as a clean bill of health. Track the Tauri
GTK migration before calling the Linux runtime hardened for hostile content.

## Create the beta

For the first beta, the version is `0.1.0`:

```powershell
git status --short
git tag -a v0.1.0 -m "release: Zest 0.1.0 beta"
git push origin v0.1.0
```

The release workflow checks that the tag, Cargo version, and Tauri version
match. It then builds:

- Windows `.msi` and `.exe` installers;
- Linux `.deb`, `.rpm`, and AppImage packages;
- one SHA256 manifest per platform;
- `LICENSE.txt` and `THIRD_PARTY_NOTICES.md`.

The workflow marks the GitHub Release as a prerelease. Do not manually upload
files from a local build as a substitute for the tagged workflow.

## Signing and clean-machine checks

Unsigned installers are valid beta artifacts but do not prove publisher
identity. If signing is enabled later, keep the private certificate in the
certificate store or signing service and publish only the public certificate
fingerprint. The signing overlay is ignored by Git.

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

Do not claim live-provider verification from compilation, unit tests, or a mock
server. Never put credentials or response contents in release notes.

## After publication

Install one artifact from the GitHub Release page, not only the local build,
then record the final download URL and checksum in the release record. If an
asset is rebuilt, rerun the workflow from a new tag or update the existing
release with `gh release upload --clobber`; never silently replace a file.
