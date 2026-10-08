---
title: Interactive HTML artifacts
summary: Agents publish self-contained interactive HTML cards in chat, with isolated execution, source/export controls, and optional real screenshot feedback.
paths:
  - crates/core/src/html.rs
  - crates/core/src/html_preview.rs
  - crates/core/src/tools/html.rs
  - crates/core/src/tools/outcome.rs
  - crates/core/src/agent.rs
  - crates/core/src/runtime.rs
  - crates/core/src/thread.rs
  - crates/core/src/provider/rig_convert.rs
  - crates/core/src/provider/openai_compatible.rs
  - crates/desktop/src/html.rs
  - crates/desktop/src/html_native_check.rs
  - crates/desktop/src/lib.rs
  - crates/desktop/src/turn.rs
  - crates/desktop/ui/src/components/HtmlArtifactCard.tsx
  - crates/desktop/ui/src/components/Markdown.tsx
  - crates/desktop/ui/src/components/ToolCallRow.tsx
  - crates/desktop/ui/src/lib/htmlArtifacts.ts
  - crates/desktop/ui/src/lib/api.ts
  - crates/desktop/ui/src/lib/backend.ts
  - crates/desktop/ui/src/lib/fixtureBackend.ts
  - crates/desktop/ui/src/lib/toolRuns.ts
  - crates/desktop/ui/vite.config.ts
  - scripts/zest-html-native-check.mjs
tests:
  - crates/desktop/ui/src/lib/htmlArtifacts.test.ts
  - crates/desktop/ui/e2e/html-artifacts.spec.ts
verify:
  - cargo test -p zest-core --lib html
  - cargo test -p zest-core --lib installed_browser -- --ignored
  - cargo test -p zest-core --lib html_agent_preview -- --ignored
  - cargo test -p zest-desktop --lib html
  - npm run ui:e2e -- html-artifacts
---

# Interactive HTML artifacts

## Behavior

- Native API desktop chats get parent-only `html_render` and, when execution tools are enabled, `html_preview`. Existing tools retain their order. Headless runtimes do not advertise a chat viewer; delegation workers receive neither tool.
- Publication stores inert title/source metadata in the conversation. The user presses Open to execute it. Buttons and local JavaScript work; external scripts, remote assets, requests, local files, workers, popups, native IPC and parent access are unavailable.
- Native interactive execution is initially Windows-only, pending equivalent native proof on other platforms. Source and Save remain usable there. This is document confinement, not a defense against browser-engine exploits or a guarantee of zero browser-background traffic.
- Claude Code, Codex CLI and Cursor emit an explicitly closed backtick or tilde `zest-html` fence, rendered by the same card. Their own loops cannot call Zest preview tools: no automatic same-turn screenshot feedback, no vendor configuration changes. Ordinary `html` fences and incomplete streaming fences remain code.
- Fence contents retain their exact source (including LF/CRLF, whitespace and Windows image-like strings in JavaScript) through Source, preview preparation and Save. Only the surrounding fence lines and their separating newlines are excluded; export adds its isolation prefix without rewriting the HTML body.
- Preview is Exec risk, never auto-eligible. It uses an installed Chromium/Edge, an empty temporary profile, a credential-free environment and controlled process tree. It returns a real 1024×768 PNG and bounded console/errors/blocked requests. An unavailable engine or failed confinement is an error, not simulated success. Capture/timeout/cancellation removes its process tree/profile.
- Preview session approval is bound to a fingerprint of the validated title and exact HTML bytes. A changed document requires fresh consent, even if its title and size match an earlier preview.
- Vision-capable native models receive actual image content; text-only/undeclared models get an explicit notice that pixels were not sent. Images are not inserted into content-free usage traces.
- Source, Reset, Expand and Save are host controls. Reopening resets interaction state. HTML export uses a user-selected `.html` path and opens outside Zest's frame sandbox; do not treat exported files as trusted code.
- Source is bounded to 512 KiB UTF-8, titles to 120 characters. Data is inline in saved chat metadata or assistant fences: forks own their copy, with no attachment files or durable viewer URLs. Thread format v5 is not compatible with old binaries; do not downgrade a workspace containing new artifact metadata without backups.

## How it works

`tools/html.rs` validates `HtmlDocument` before approval/execution. The agent carries typed screenshot blocks independently of text spilling; provider adapters preserve tool-call IDs and image content. `html_preview.rs` synthesizes exact one-time responses through Chromium's request interception and denies other requests, while the child and trusted wrapper have restrictive response policies.

The Markdown local-image pass skips fenced contents until a matching closing marker of sufficient length. `htmlDocumentFromFence` matches fence lines without normalizing the HTML body, so prose image rewriting cannot insert Markdown into artifact scripts or alter path strings.

The card requests a bounded, expiring 256-bit random lease from `prepare_html_view`. The dedicated `zest-html` protocol serves only those in-memory bytes with an authoritative CSP response header and no path lookup. Host CSP narrowly allows that frame origin; release host script policy stays unchanged. The iframe has only `allow-scripts`, no same-origin or host bridge. The prefix disables WebRTC constructors before generated code runs. Leases are released on unmount/reset, cannot be persisted as artifact identities, and expire after 30 minutes.

All application commands reject non-main/untrusted host contexts, supplementing Tauri's main-frame-only IPC initialization and invoke-key checks. The debug-only native test runtime never loads user config, credentials, project paths or chats.

## Verify

Run focused commands above, regenerate DTOs normally, then `npm run verify`. Browser fixture checks are not native IPC proof. On Windows, build the debug desktop binary and run `scripts/zest-html-native-check.mjs` with `ZEST_HTML_NATIVE_CHECK=1`; its default guarded main-host driver exercises the actual WebView2 protocol/command path without needing an externally reachable debugger. Only a closed, content-free result schema is written into its private temporary directory; foreign artifact messages do not become commands. Evidence stays in temporary/build locations, not repository history. Installed-browser tests are opt-in and require no paid API calls.

Native host-driver interaction uses scripted button clicks without user activation. It verifies actual protocol headers, sibling/nested-realm isolation, IPC attempts, resource denials, independent network canaries and revocation. It has no native compositor screenshot, console observer or local-file response observer; these are unknown, not zero. Real-click activation-sensitive cases are covered by browser fixtures, while the separate installed-browser agent test proves actual PNG/error feedback. Optional `ZEST_HTML_NATIVE_CHECK_DRIVER=cdp` adds native screenshot/console observation when WebView2 exposes its debugger; do not substitute a debugger startup failure for a passing test.

The persistent control CLI supports `inspect html`, `html-click "Button"`, and `html-value "Label" "Value"`. Fixture mode uses dedicated HTTP responses with the same child policy, never `srcdoc`. Reloading a seeded fixture proves UI restoration; Rust tests prove disk/fork/delete ownership separately.

`htmlArtifacts.test.ts` covers exact-source extraction through the Markdown pipeline for backtick/tilde fences and LF/CRLF. The HTML browser spec emits raw fixture responses (avoiding textarea newline normalization), verifies Source equality, runs JavaScript containing Windows image-like strings, and checks that preview responses and downloads retain the exact HTML suffix with confinement still enabled.
