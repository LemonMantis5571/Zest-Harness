---
title: Interactive HTML artifacts
summary: Agents publish self-contained interactive HTML cards in chat, with isolated execution, source/export controls, and optional real screenshot feedback.
paths:
  - crates/desktop/src/artifact.rs
  - crates/desktop/src/web_content.rs
  - crates/desktop/src/web_window.rs
  - crates/desktop/src/native.rs
  - crates/desktop/src/native_view.rs
  - crates/core/src/html.rs
  - crates/core/src/html_preview.rs
  - crates/core/src/tools/html.rs
  - crates/core/src/tools/outcome.rs
  - crates/core/src/agent.rs
  - crates/core/src/runtime.rs
  - crates/core/src/thread.rs
  - crates/core/src/provider/rig_convert.rs
  - crates/core/src/provider/openai_compatible.rs
tests:
  - docs/gpui-spike/benchmark/web-windows.mjs
verify:
  - cargo test -p zest-core --lib html
  - cargo test -p zest-core --lib installed_browser -- --ignored
  - cargo test -p zest-core --lib html_agent_preview -- --ignored
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- artifact::tests
---

# Interactive HTML artifacts

## Behavior

- Native API desktop chats get parent-only `html_render` and, when execution tools are enabled, `html_preview`. Existing tools retain their order. Headless runtimes do not advertise a chat viewer; delegation workers receive neither tool.
- Publication stores inert title/source metadata in the conversation. The card shows the title with Open, Source and Save HTML. The user presses Open to execute it in a separate viewer window. Buttons and local JavaScript work; external scripts, remote assets, requests, local files, workers, popups, native IPC and parent access are unavailable.
- Interactive execution is Windows-only, pending equivalent native proof on other platforms: elsewhere the card has no Open button, and Source and Save HTML remain usable. This is document confinement, not a defense against browser-engine exploits or a guarantee of zero browser-background traffic.
- Claude Code, Codex CLI and Cursor emit an explicitly closed backtick or tilde `zest-html` fence, rendered by the same card. Their own loops cannot call Zest preview tools: no automatic same-turn screenshot feedback, no vendor configuration changes. Ordinary `html` fences and incomplete fences remain code. A fence's card appears once the turn ends and the answer is rebuilt from the saved message; a tool-published card appears as soon as the tool result arrives.
- Fence contents retain their exact source (including LF/CRLF, whitespace and Windows image-like strings in JavaScript) through Source, the viewer and Save. Only the surrounding fence lines and their separating newlines are excluded; export adds its isolation prefix without rewriting the HTML body.
- Preview is Exec risk, never auto-eligible. It uses an installed Chromium/Edge, an empty temporary profile, a credential-free environment and controlled process tree. It returns a real 1024×768 PNG and bounded console/errors/blocked requests. An unavailable engine or failed confinement is an error, not simulated success. Capture/timeout/cancellation removes its process tree/profile.
- Preview session approval is bound to a fingerprint of the validated title and exact HTML bytes. A changed document requires fresh consent, even if its title and size match an earlier preview.
- Vision-capable native models receive actual image content; text-only/undeclared models get an explicit notice that pixels were not sent. Images are not inserted into content-free usage traces.
- Source toggles a scrollable view of the HTML under the card ("Hide source" closes it); one card's source is open at a time. The viewer window has a "Reset document" link that reloads the document. Each Open starts a fresh document with no saved interaction state. Save HTML asks for a path, forces the `.html` extension, and writes the document with its CSP meta tag; exported files open outside Zest's sandbox, so do not treat them as trusted code.
- Source is bounded to 512 KiB UTF-8, titles to 120 characters. Data is inline in saved chat metadata or assistant fences: forks own their copy, with no attachment files or durable viewer URLs. Thread format v5 is not compatible with old binaries; do not downgrade a workspace containing new artifact metadata without backups.

## How it works

`tools/html.rs` validates `HtmlDocument` before approval/execution. The agent carries typed screenshot blocks independently of text spilling; provider adapters preserve tool-call IDs and image content. `html_preview.rs` synthesizes exact one-time responses through Chromium's request interception and denies other requests, while the child and trusted wrapper have restrictive response policies. The desktop enables the tools with `RuntimeBuilder::with_html_artifacts(true)` in `native.rs` `Chat::agent`; preview approval goes through the same approval card as other tools.

Cards come from two places. `artifact::metadata_document` turns a tool result's `ToolMetadata::HtmlDocument` into a card, live (`EvKind::ToolResult` in `native.rs` `apply`) and when a chat is reopened (`append_saved_rows` in `native_view.rs`). `artifact::fenced_documents` scans a saved answer for `zest-html` fences: it takes the title from the opening line, requires a closing marker of the same character and at least the opening length, and keeps the body bytes unchanged. `artifact::card` draws the card.

Open calls `web_content::show_artifact`, which launches the Zest executable again with `--zest-artifact-window`: cleared environment (display and runtime variables only, no provider tokens), a temporary working directory and webview profile, and the document sent as one JSON line on stdin. `web_window.rs` builds a wry webview serving a wrapper page and the document from a custom `zest-artifact` protocol (`http://zest-artifact.localhost` on Windows) under a random 256-bit path. Only GET requests without a query to those two paths succeed, for 30 minutes; the document carries `HTML_CSP` as a response header, the wrapper's iframe has only `allow-scripts`, and navigation is limited to the two URLs. The prefix from `HtmlDocument::isolated_html` disables WebRTC constructors before generated code runs. The viewer has no host IPC handler. Open fails if the frame has not loaded within 20 seconds; the child exits when its window closes or Zest closes its pipe.

## Verify

Run the focused commands above. `artifact::tests` cover exact-source fence extraction with CRLF and a title, and that unclosed, plain `html` and nested example fences stay inert. Installed-browser tests are opt-in and require no paid API calls.

On Windows, build the release binary and run `node docs/gpui-spike/benchmark/web-windows.mjs PATH_TO_BINARY`; it drives the real browser and artifact helper windows against a local fixture, including a document with a button. To check by hand, publish an artifact with a configured provider, then use Open, Source and Save HTML and confirm the saved file matches the source after the CSP prefix.

## Pitfalls

- A streaming answer has no cards; they are computed from the saved text when the turn ends, so an unclosed fence never becomes a card mid-stream.
- The viewer runs in a separate process on purpose. Do not add an IPC handler, a file protocol, or navigation beyond the wrapper and document URLs.
- Diagram and math cards use the same window in `RENDER` mode (see [Message rendering](message-rendering.md)). Its only extra route is `/{token}/vendor/`, which serves the embedded mermaid and KaTeX files with `Access-Control-Allow-Origin: *` because the sandboxed frame fetches KaTeX fonts in CORS mode. HTML artifacts never get that route or its policy.
- Toggling Source changes a row's height; `artifact::card` remeasures the old and new rows.
