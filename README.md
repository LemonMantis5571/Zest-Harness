<div align="center">

<img src="./crates/desktop/ui/src/assets/zest-mark.png" alt="Zest" width="72" height="72" />

# Zest

[![Windows verify](https://github.com/LemonMantis5571/Zest-Harness/actions/workflows/windows-verify.yml/badge.svg?branch=master)](https://github.com/LemonMantis5571/Zest-Harness/actions/workflows/windows-verify.yml)
[![Linux verify](https://github.com/LemonMantis5571/Zest-Harness/actions/workflows/linux-verify.yml/badge.svg?branch=master)](https://github.com/LemonMantis5571/Zest-Harness/actions/workflows/linux-verify.yml)
[![Latest beta](https://img.shields.io/github/v/release/LemonMantis5571/Zest-Harness?include_prereleases&label=latest%20beta)](https://github.com/LemonMantis5571/Zest-Harness/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**A local-first coding harness for agent orchestration, with explicit delegation.**

Keep the parent session on one provider. Hand bounded work to a worker. Review the result in a fresh workspace.

[Install the beta](https://github.com/LemonMantis5571/Zest-Harness/releases) · [Build from source](#build-from-source) · [Docs](#docs)

<img src="./crates/desktop/ui/src/assets/hero.png" alt="" width="320" />

</div>

## What Zest is

Zest is a coding harness. The parent session stays with the provider you picked: it reads the project, runs the agent loop, and keeps that transcript recoverable. Desktop and terminal both sit on the same core.

Orchestration is the point. When a task is better handled elsewhere, Zest does not switch the parent provider or fold another model into the same chat. It delegates.

## Delegation

A delegated job is a **feature card**: objective, scope, selected context, dependencies, acceptance checks, worker target, and reviewer target. The coordinator owns that card, the queue, retries, and apply/review transitions. It is not a second parent agent.

Two lanes share those records, not credentials or parent history:

- **Native provider workers** run through Zest's own provider and runtime boundary.
- **External workers** run through a configured ACP session or a signed-in CLI that already owns its own login.

The parent conversation stays with its provider. Workers do not inherit the parent transcript. External CLIs keep their own credentials and sessions. Review happens in a fresh, isolated workspace; reviewer edits are discarded, and only a validated review report can make a job ready to apply.

Delegation is opt-in and configured. Zest does not auto-route tasks between providers, implement vendor OAuth for workers, or manage a worker's own MCP servers.

## Local-first

No Zest account and no telemetry to a Zest server. Config lives at `~/.zest`. Provider credentials use the OS credential manager when available. Usage is recorded honestly per provider; external-CLI spend is never invented or merged into the parent balance.

The parent can be a native API, an OpenAI-compatible endpoint, or an authenticated coding CLI. Optional plugins install as separate processes. See [Plugins](docs/PLUGINS.md) and [quota](docs/QUOTA.md).

## Install the beta

Packages live on [GitHub Releases](https://github.com/LemonMantis5571/Zest-Harness/releases).

- **Windows 10/11 x64.** `.msi` or `.exe`
- **Linux x64.** `.deb`, `.rpm`, or AppImage

Each release includes `SHA256SUMS` and third-party notices. Official packages do not bundle plugins.

### First session

1. Launch Zest.
2. Choose a parent provider in **Settings**.
3. Open a project folder.
4. Work in the parent session, or create a feature card and delegate.

## Build from source

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the pinned toolchain, Linux packages, and local verification gate.

## Platforms

| Platform | Status |
| --- | --- |
| Windows 10/11 x64 | Beta installer and CI-verified |
| Linux x64 | Beta packages and CI-verified |
| Windows/Linux ARM64 | Source builds only |
| macOS | Source paths exist; CI and installers are not available yet |

## Security

Writes, commands, and delegated jobs are gated before they run. An approved command still runs with your OS permissions; Zest is not an operating-system sandbox. Plugins are separate child processes, not a sandbox either. Install only plugins you trust.

## Docs

- [Plugins](docs/PLUGINS.md). Install, build, protocol, and review standard.
- [Skills](docs/SKILLS.md). Personal skills and install locations.
- [MCP servers](docs/MCP.md). Zest's own servers and how they differ from `allow_mcp`.
- [Provider quota](docs/QUOTA.md). Live limits, balances, and local usage.
- [Contributing](CONTRIBUTING.md). Development, tests, and pull requests.
- [Contributors](CONTRIBUTORS.md). People who have contributed to Zest.
- [Releasing](docs/RELEASING.md). Maintainer release checklist.
- [Changelog](CHANGELOG.md). User-facing changes.
- [Security policy](SECURITY.md). Vulnerability reporting.
- [Third-party notices](THIRD_PARTY_NOTICES.md). Dependency attribution.

## Contributors

- [LemonMantis5571](https://github.com/LemonMantis5571)
- [rjamador](https://github.com/rjamador)

## Contributing

Read [`CONTRIBUTING.md`](CONTRIBUTING.md), keep each change scoped to one
behavior, and include tests for behavior changes. Report security
vulnerabilities privately using
[`SECURITY.md`](SECURITY.md).

Zest is released under the [MIT License](LICENSE).

## Inspiration

Zest is inspired by Comet's branch-diff review, T3 Cursor's provider and session separation, and DeepSeek Harness's chat workflow. Its desktop UI uses shadcn/ui and ReUI components, restyled with Zest's own design tokens.
