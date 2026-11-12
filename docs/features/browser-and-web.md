---
title: Local browser and web search
summary: The agent can drive a Zest browser window (open, read, click, type) behind approvals, and search the public web through DuckDuckGo without an API key.
paths:
  - crates/core/src/tools/browser.rs
  - crates/core/src/tools/web_search.rs
  - crates/desktop/src/browser.rs
verify:
  - cargo test -p zest-core --lib -- tools::browser
  - cargo test -p zest-core --lib -- tools::web_search
  - cargo test -p zest-desktop --lib -- browser::tests
---

# Local browser and web search

## Behavior
- `browser` actions: `open` (needs `url`), `snapshot`, `click` / `type` (need a
  `locator`; `type` needs `text`), `press` (needs `key`), `wait`.
- Only `http` and `https` URLs are accepted, both in request validation and in
  the desktop adapter; the window's navigation handler also blocks other schemes
  for redirects and link clicks.
- Locators are `css`, or `role` / `name` / `text` with optional `index`; at least
  one field is required. No coordinates are exposed.
- Risk per action: `open` and `wait` are Read (no prompt); `snapshot` is
  Sensitive (asks every time except in Bypass, result hidden from the UI summary
  and redacted from persisted history); `click`, `type`, `press` are Exec.
- Limits: URL 4096 chars, typed text 20000, snapshot text default 12000 chars
  (max 20000), up to 120 visible interactive elements, timeout default 8 s
  (max 30 s).
- The approval card for `type` names the locator, never the typed text.
- The browser tool is parent-only: delegated workers never get it, and it is not
  registered when a CLI provider owns the agent loop.
- `web_search` takes `query` (max 400 chars) and `max_results` (1-8, default 5),
  POSTs to `html.duckduckgo.com/html/` with a 15 s timeout, and returns
  numbered title / URL / snippet lines, or "No web results for: ...". It is Read
  risk and is also available to delegated workers.

## How it works
1. Desktop `start_session` (crates/desktop/src/lib.rs) passes
   `state.browser.adapter()` to the runtime builder; `BrowserHost` is attached to
   the Tauri app handle in setup. Each adapter owns one webview window labelled
   `zest-browser-N`, created lazily on the first `open` and closed when the
   adapter drops.
2. runtime.rs calls `register_browser_tool` after cloning `worker_tools`, and
   appends `LOCAL_BROWSER_SYSTEM` to the parent's system prompt.
3. `BrowserTool::prepare` parses untrusted JSON with `BrowserRequest::from_value`
   (`deny_unknown_fields`, `validate`) and sets per-action risk and an approval
   preview (URL or `BrowserLocator::describe`).
4. `LocalBrowserAdapter::execute` serializes calls with a per-adapter mutex (the
   agent may run Read-risk calls concurrently) and evaluates injected scripts:
   `snapshot` collects `innerText` plus interactive elements; `locator_script`
   provides `findElement` / `elementInfo` for click, type, press, wait.
5. `WebSearch::run` -> `search_duckduckgo` -> `parse_ddg_html` (regex over the
   result HTML; `decode_ddg_href` unwraps `uddg=` redirect links).

## Verify
Unit tests cover request validation, risk mapping, preview redaction, locator
escaping in injected scripts, URL schemes, and DuckDuckGo HTML parsing. The real
webview only exists in the Tauri app; fixture mode does not provide a browser.

## Pitfalls
- A snapshot can contain an authenticated page; keep it Sensitive so it stays
  out of durable history and spill files.
- `type` sets the value through the native setter and dispatches `input` and
  `change`; `press Enter` also calls `form.requestSubmit()`.
- `web_search` scrapes DuckDuckGo's HTML; markup changes break parsing silently
  (it then reports no results rather than an error).
