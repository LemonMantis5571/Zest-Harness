---
title: Local browser and web search
summary: The agent can drive a Zest browser window (open, read, click, type) behind approvals, and search the public web through DuckDuckGo without an API key.
paths:
  - crates/core/src/tools/browser_dom.rs
  - crates/desktop/src/web_content.rs
  - crates/desktop/src/web_window.rs
  - crates/core/src/tools/browser.rs
  - crates/core/src/tools/web_search.rs
verify:
  - cargo test -p zest-core --lib -- tools::browser
  - cargo test -p zest-core --lib -- tools::web_search
---

# Local browser and web search

## Behavior
- `browser` actions: `open` (needs `url`), `snapshot`, `click` / `type` (need a
  `locator`; `type` needs `text`), `press` (needs `key`), `wait`.
- Only `http` and `https` URLs with a host are accepted, both in request
  validation and in the browser window; the window's navigation handler also
  blocks other schemes for redirects and link clicks.
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
- In the desktop app each chat has its own browser window ("Zest Browser",
  1080 x 760), opened on the chat's first `open`. Any other action before that
  fails with "No browser page is open. Open a URL first." The window runs as a
  separate process with a throwaway profile: no devtools, no clipboard access,
  every permission request denied, no new windows, no downloads, and on Windows
  no default context menus. `open` replies once the page has finished loading.
- A failed or timed-out action ("Browser action timed out") closes that chat's
  browser window, so a late reply cannot be read as the answer to the next
  call; the next `open` starts a fresh window. Closing the window by hand has
  the same effect. The window also closes when its chat is dropped.
- `web_search` takes `query` (max 400 chars) and `max_results` (1-8, default 5),
  POSTs to `html.duckduckgo.com/html/` with a 15 s timeout, and returns
  numbered title / URL / snippet lines, or "No web results for: ...". It is Read
  risk and is also available to delegated workers.

## How it works
1. Each desktop `Chat` (`crates/desktop/src/native.rs`) holds an
   `Arc<web_content::Browser>` and passes it to `RuntimeBuilder::with_browser_adapter`.
2. runtime.rs calls `register_browser_tool` after cloning `worker_tools`, and
   appends `LOCAL_BROWSER_SYSTEM` to the parent's system prompt.
3. `BrowserTool::prepare` parses untrusted JSON with `BrowserRequest::from_value`
   (`deny_unknown_fields`, `validate`) and sets per-action risk and an approval
   preview (URL or `BrowserLocator::describe`).
4. `Browser::execute` (`web_content.rs`) serializes calls with a per-chat mutex
   (the agent may run Read-risk calls concurrently). On the first `open`,
   `Session::launch` starts the Zest executable with `--zest-browser-window`,
   a cleared environment (display, locale and packaged-library variables only),
   and home, app-data and temp variables pointed at a temporary profile, so
   provider tokens never reach the web process. Requests and replies are one JSON
   line each over the child's stdin and stdout (4 MiB per request, 2 MiB per
   reply). `wait` resends every 100 ms until the reply says `ready` (or `found`
   with a locator). The session is kept only after a complete, successful
   exchange.
5. In the child, `web_window::dispatch` (called first in `main.rs`) runs `run`:
   a tao window with a wry webview. `open` rebuilds the webview with a navigation
   handler that allows only external HTTP(S) URLs. Other actions evaluate the
   script from `zest_core::tools::browser_dom::page_action` and answer with
   `decode_eval_result`: `snapshot` collects `innerText` plus interactive
   elements, and the locator helpers serve click, type, press and wait. The
   child exits when its stdin closes or its window is closed.
6. `WebSearch::run` -> `search_duckduckgo` -> `parse_ddg_html` (regex over the
   result HTML; `decode_ddg_href` unwraps `uddg=` redirect links).

## Verify
Unit tests cover request validation, risk mapping, preview redaction, locator
escaping in injected scripts, URL schemes, and DuckDuckGo HTML parsing. To drive
the real web window, build the desktop app and run `node
docs/gpui-spike/benchmark/web-windows.mjs PATH_TO_BINARY`: it opens a local
fixture page with `--zest-browser-window`, checks the `open` reply and that a
snapshot contains the page text, and checks that the window exits when its
input closes. On Windows it also opens an HTML document with
`--zest-artifact-window`.

## Pitfalls
- A snapshot can contain an authenticated page; keep it Sensitive so it stays
  out of durable history and spill files.
- `type` sets the value through the native setter and dispatches `input` and
  `change`; `press Enter` also calls `form.requestSubmit()`.
- Keep the web process's environment cleared. It runs untrusted pages, so it
  must not inherit provider keys or the user's real profile directories.
- Because any failed action drops the session, a locator that does not match
  also closes the page. The agent has to `open` again before its next action.
- `web_search` scrapes DuckDuckGo's HTML; markup changes break parsing silently
  (it then reports no results rather than an error).
