---
title: Message rendering
summary: Assistant answers render as Markdown with highlighted code, tables, selectable text, safe links, and copy/save actions, even while they stream.
paths:
  - crates/desktop/src/md.rs
  - crates/desktop/src/view.rs
  - crates/desktop/src/main.rs
  - crates/desktop/src/native_view.rs
  - crates/desktop/src/theme.rs
tests:
  - crates/desktop/src/selftest.rs
  - crates/desktop/src/selftest_trace.rs
verify:
  - npm run gpui -- --selftest
  - npm run gpui -- --selftest --agent
  - npm run gpui -- --langs
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- native_view::tests
---

# Message rendering

## Behavior
- The work behind an answer renders above it as one collapsible row per step, in the order it happened: a thinking block is a "Thinking" row (relabelled "Thought for Ns" when it ends), a tool call is a row with the tool's name, the first line of its result, and "running…" or its duration, and provider activity (Claude Code, Codex, Cursor) is a "Provider" row. A failed row is red with an X icon. Rows start collapsed; clicking one opens its body (the reasoning text, or the tool result or diff as a code block).
- A reopened chat rebuilds the same rows from the saved message's `steps`; reasoning rows then read "Thought" without a duration.
- While a reply streams, "Stream complete blocks" (Customize > Chat view, on by default) shows only the Markdown blocks that are finished: `md::complete_prefix` holds back the paragraph, list or code fence still being written, so half-written tables and fences do not flicker. Off, every token is rendered as it arrives.
- The desktop app does not show framed subagent lines, nested activity under a parent, or a "Waiting for approval" row; delegation progress lives in the Delegation board and a pending approval is only the card above the composer.
- Explicitly closed `zest-html` fences and completed native HTML tool results show interactive artifact cards; ordinary HTML code stays inert. See [Interactive HTML artifacts](html-artifacts.md).
- Assistant text renders as GitHub-flavoured Markdown: headings, paragraphs, block quotes, ordered and bullet lists (nested), task-list boxes, tables, strikethrough, rules, inline code and fenced code. Raw HTML is shown as text. User bubbles are plain text.
- Only `http(s)` links are followed: clicking one opens the system browser. Other schemes are coloured like links but do nothing. There are no link-preview cards.
- Fenced code shows an uppercase language chip ("TEXT" when the fence has no tag) and a "Copy code" button, and scrolls horizontally instead of wrapping. Highlighting uses syntect: the default pack, or the larger bat/two-face pack with `--features grammars`. A few fence spellings map to syntect tokens (`csharp`, `docker`, `kotlin`, `typescript`, `sh`/`shell`/`zsh`, `ps1`, `rs`, `py`); TypeScript, TSX and JSX fall back to the JavaScript grammar and PowerShell to Bash. A tag with no grammar keeps its label and renders uncoloured. Code colours follow the theme's brightness for newly rendered messages.
- The desktop app does not render Mermaid diagrams, math, or images: a Mermaid fence is a code block and a Markdown image shows its alt text as a link.
- Text in any row can be selected by dragging, across messages and off-screen rows, and copied with Ctrl+C. Collapsed rows are left out of the copy.
- A streaming answer shows a block cursor at its end. Each finished answer shows "Copy message" (the rendered text of its blocks, separated by blank lines, not the raw Markdown source) and "Download message" (a save dialog starting in the project folder with `zest-reply.md`; `.md` is added when the name has no extension). Neither shows while the answer streams.
- "Zest" labels the first row of each assistant turn. The transcript is a centered 720 px column that follows the newest row until the user scrolls up.

## How it works
- `main.rs` `Root::apply` turns bridge events into rows (`Msg`): `Thinking` opens a live reasoning row, `ToolStart`/`ToolResult` add and settle tool rows, `Activity` goes to `update_activity` (`native_view.rs`), and `Text` appends to `live_text`. After each event batch, `reparse` runs `md::parse` over the whole live answer; `End` reparses once more, then `restore_native` rebuilds the turn's rows from the saved thread (`append_saved_rows` in `native_view.rs`).
- `main.rs` `Root::reparse` parses `md::complete_prefix(live_text)` for the streaming message when the setting is on; `complete_prefix` cuts at the last blank line outside a fence, keeping list items together.
- `md.rs` `parse` walks `pulldown-cmark` events (tables, strikethrough and task lists enabled) into a flat `Doc` of `Block`s and inline `Unit`s with spans (bold, italic, strike, code, link, colour). Code blocks call `highlight`, which caches spans by language, source and theme brightness and resolves grammars with `find_syntax`.
- `view.rs` draws blocks (`render_blocks`), user bubbles (`render_user`), answers (`render_assistant`) and step rows (`render_trace`, with `trace_icon` choosing the icon). Each unit is a selectable text element registered in the shared selection state.
- `main.rs` `selection_text` builds the copied text from units and their separators; `click_link` opens `http(s)` links with `cx.open_url`; `render_item` adds the "Zest" label, artifact cards and the Copy/Download buttons; `save_message` writes the reply with `zest_core::atomic_write`.
- `npm run gpui -- --langs` prints which of Zest's fence languages the loaded pack colours natively, approximately, or not at all.

## Verify
- `npm run gpui -- --selftest` opens the rendering fixture and reports tail following, scroll release, copy across off-screen messages, link clicks, Enter to send, and paste.
- `npm run gpui -- --selftest --agent` checks reasoning and tool rows: live row order, result titles, and that expanding a row keeps the tail pinned or stays in place when scrolled up.
- The demo provider answers with fixed text, so check a fence, a table and a link end to end with a configured provider (`npm run gpui -- --project <path>`), asking for an answer that contains them.

## Pitfalls
- The live answer is reparsed in full on every event batch; keep `md::parse` cheap and rely on the highlight cache for unchanged code blocks.
- A collapsed row has no body on screen, so reasoning text is rebuilt per token only while its row is open; `finish_think` writes the final text when the block ends.
- Row heights are measured lazily. Any change to a row's content (toggle, tool result, artifact source) must call `list.remeasure_items` for that row.
- Copy message copies rendered text, so Markdown syntax such as `**` or fence markers is not included.
