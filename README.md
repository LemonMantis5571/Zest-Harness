<div align="center">

<img src="./crates/desktop/ui/src/assets/zest-mark.png" alt="Zest logo" width="256" height="256" />

# Zest

[![Windows verify](https://github.com/LemonMantis5571/Zest/actions/workflows/windows-verify.yml/badge.svg?branch=master)](https://github.com/LemonMantis5571/Zest/actions/workflows/windows-verify.yml)
[![Linux verify](https://github.com/LemonMantis5571/Zest/actions/workflows/linux-verify.yml/badge.svg?branch=master)](https://github.com/LemonMantis5571/Zest/actions/workflows/linux-verify.yml)
[![Latest beta](https://img.shields.io/github/v/release/LemonMantis5571/Zest?include_prereleases&label=latest%20beta)](https://github.com/LemonMantis5571/Zest/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**A local-first coding workspace with approvals, diffs, quotas, and optional model delegation.**

Run AI-assisted work in your own projects while keeping files, commands,
credentials, and provider accounts under local control.

[Install the beta](#how-do-i-install-zest) · [Build from source](#how-do-i-build-zest-from-source) · [Documentation](#where-can-i-find-the-docs)

</div>

## What is Zest?

Zest is a desktop and terminal coding workspace. It connects to supported model
providers, opens a project folder, and shows proposed file changes and commands
before they run.

Zest runs locally. It does not require a Zest account or send telemetry to a
Zest server. Provider sign-ins remain with their providers, and external coding
CLIs remain responsible for their own sessions.

## Influences and UI foundations

Zest is inspired by Comet, T3 Cursor, and DeepSeek Harness. Its desktop UI uses
shadcn/ui and ReUI source components, adapted to Zest's local-first workflow and
dark design system.

## What can Zest do?

- **Reviewable changes** — inspect diffs and approve file writes or commands.
- **Desktop and terminal clients** — use the Tauri desktop app or the `zest`
  terminal client.
- **Provider choice** — use supported sign-ins, native APIs, or
  OpenAI-compatible endpoints.
- **Local usage** — track requests and tokens without presenting local usage as
  a provider balance.
- **Live quota data** — show provider-reported limits when an official,
  supported source is available.
- **Resumable sessions** — keep project chats and checkpoints across restarts.
- **Optional delegation** — create a durable, project-local feature card, send
  it to either a configured native provider worker or an external coding CLI,
  review the returned diff in a fresh workspace, and apply it only after
  approval.
- **Optional plugins** — add local integrations without rebuilding Zest.

## How do I install Zest?

### Download the beta

Open the [latest Zest beta release](https://github.com/LemonMantis5571/Zest/releases/latest).

- **Windows x64** — install the `.msi` or `.exe` package.
- **Linux x64** — install the `.deb` or `.rpm` package, or run the AppImage.

Each release includes a platform-specific `SHA256SUMS` file and third-party
notices.

### Start a first session

1. Launch Zest.
2. Choose a provider in **Settings**.
3. Open a project folder.
4. Start a chat and inspect the proposed changes or commands shown by Zest.

## How do I build Zest from source?

Install Rust 1.97.1, Node.js 24.16.0, npm, Git, and PowerShell (`pwsh` on
Linux or macOS). Linux also needs the desktop libraries listed in
[`CONTRIBUTING.md`](CONTRIBUTING.md).

### Windows PowerShell

```powershell
npm ci
npm run dev
```

### Linux or macOS

```bash
npm ci
npm run dev
```

Build the terminal client with Cargo:

```bash
cargo run -p zest -- --help
```

Run the full local verification gate with:

```powershell
.\scripts\release-verify.ps1
```

## How do I install an optional plugin?

Official Zest packages do not include plugins. Plugins are installed separately
as folders under the user's Zest plugin directory.

1. Open **Customize > Extras**.
2. Press **Open folder**.
3. Copy one complete plugin folder into the folder that opens.
4. Press **Refresh**, then **Turn on**.

On Windows, the folder is:

```text
%LOCALAPPDATA%\Zest\plugins
```

Install the included Windows music plugin from the repository root with:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\install-now-playing-plugin.ps1
```

The complete install guide, plugin protocol, security rules, and acceptance
checklist are in [`docs/PLUGINS.md`](docs/PLUGINS.md).

## How do I configure providers and quotas?

Zest stores user configuration at `~/.zest/zest.toml`. Supported provider
credentials use the operating system credential manager when available; do not
put secrets in project files or commit them to Git.

Zest keeps these values separate:

- local usage recorded by Zest;
- rate limits returned by a provider; and
- account balances or subscription limits owned by a provider.

The quota panel only shows provider-reported data. See the
[quota guide](docs/QUOTA.md) for provider-specific behavior.

## Which platforms are supported?

| Platform | Status |
| --- | --- |
| Windows 10/11 x64 | Beta installer and CI-verified |
| Linux x64 | Beta packages and CI-verified |
| Windows/Linux ARM64 | Source builds only |
| macOS | Source paths exist; CI and installers are not available yet |

## What are the security limits?

Zest's approval layer makes proposed writes and commands visible before
execution. An approved command still runs with the user's operating-system
permissions; Zest is not an operating-system sandbox.

Plugins run as separate child processes, but the plugin boundary is not a
sandbox either. Install only plugins whose source and behavior are trusted.

Delegation is explicit and local-first. The selected parent provider remains
the owner of the parent conversation; delegation does not silently switch the
parent provider or inject the parent transcript into a worker. External CLIs
keep their own credentials and sessions. The coordinator owns the project-local
job boundary, queue and retry rules, reviewer isolation, and usage accounting
model.

## Where can I find the docs?

- [Plugins](docs/PLUGINS.md) — install, build, protocol, and review standard
- [Skills](docs/SKILLS.md) — personal skills and install locations
- [MCP servers](docs/MCP.md) — Zest's own servers, and how they differ from `allow_mcp`
- [Provider quota](docs/QUOTA.md) — live limits, balances, and local usage
- [Contributing](CONTRIBUTING.md) — development, tests, and pull requests
- [Design notes](DESIGN.md) — product and architecture context
- [Releasing](docs/RELEASING.md) — maintainer release checklist
- [Beta release notes](docs/releases/0.1.0.md) — scope and known limits
- [Changelog](CHANGELOG.md) — user-facing changes
- [Security policy](SECURITY.md) — vulnerability reporting
- [Third-party notices](THIRD_PARTY_NOTICES.md) — dependency attribution

## How do I contribute?

Read [`CONTRIBUTING.md`](CONTRIBUTING.md), keep changes focused, and include
tests for behavior changes. Report security vulnerabilities privately using
[`SECURITY.md`](SECURITY.md).

Zest is released under the [MIT License](LICENSE).
