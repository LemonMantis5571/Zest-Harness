# Generated desktop DTOs (ts-rs)

Rust is the source of truth for wire shapes in `crates/desktop/src/lib.rs`.
Regenerate after changing those types:

```powershell
$env:CARGO_TARGET_DIR = (Resolve-Path .\target).Path
cargo test -p zest-desktop --features export-bindings --lib export_bindings
```

`TS_RS_EXPORT_DIR` is set in repo `.cargo/config.toml` to this directory.

Committed files:

- `ChatEvent.ts` — tagged union (`kind` + snake_case variants), optional `metadata`
- `SessionInfo.ts` — camelCase session snapshot with Rust `models` / `defaultModel`
- `OlderThreadMessages.ts` — paged older turns for a windowed open
- `ProviderView.ts` — picker row with configured + catalogue
- `ModelCapability.ts` — model id + efforts
- `ToolMetaView.ts` — delegation provenance side-channel
- `DelegationJobView.ts` / `DelegationEvent.ts` — coordinator job board and lifecycle events
- `DelegationStatus.ts` / `AcceptanceCheckView.ts` / `ReviewFinding.ts` — review pipeline state

`GitContext.ts` / `PullRequestView.ts` expose the active Git checkout and optional PR metadata.

`WorkspaceReview.ts` is the read-only Git workspace review result.

`McpServerView.ts` / `McpCheckView.ts` are Zest's own MCP servers and the result
of checking one. Distinct from `ExternalAgentView.mcpAllowed`, which is a CLI
worker's permission to use servers Zest never sees.

`ChatMessage` / `ToolPart` / `UsageSnapshot` stay handwritten in `../types.ts`
(UI projection + usage command). App code imports from `../types.ts`.
