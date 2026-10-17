---
title: Message rendering
summary: Assistant answers render as Markdown with highlighted code, Mermaid diagrams, zoomable images, safe links, and copy/save actions, even while they stream.
paths:
  - crates/desktop/ui/src/components/AssistantTrace.tsx
  - crates/desktop/ui/src/components/ActivityLine.tsx
  - crates/desktop/ui/src/components/TraceHoverTitle.tsx
  - crates/desktop/ui/src/components/SubagentLine.tsx
  - crates/desktop/ui/src/components/ui/badge.tsx
  - crates/desktop/ui/src/components/ui/trace.tsx
  - crates/desktop/ui/src/components/ui/code-block.tsx
  - crates/desktop/ui/src/lib/assistantTrace.ts
  - crates/desktop/ui/src/lib/subagents.ts
  - crates/desktop/ui/src/components/Markdown.tsx
  - crates/desktop/ui/src/components/MarkdownActions.tsx
  - crates/desktop/ui/src/components/CodeBlock.tsx
  - crates/desktop/ui/src/components/MermaidBlock.tsx
  - crates/desktop/ui/src/components/ZoomableImage.tsx
  - crates/desktop/ui/src/components/reui/code-block/code-block.tsx
  - crates/desktop/ui/src/components/reui/code-block/code-block-highlight.tsx
  - crates/desktop/ui/src/lib/markdownBlocks.ts
  - crates/desktop/ui/src/lib/markdownExport.ts
  - crates/desktop/ui/src/lib/codeLanguage.ts
  - crates/desktop/ui/src/lib/linkify.tsx
  - crates/desktop/ui/src/lib/safeHtml.ts
  - crates/desktop/ui/src/lib/externalLinks.ts
  - crates/desktop/ui/src/lib/imageSrc.ts
  - crates/desktop/ui/src/lib/chatImageSrc.ts
  - crates/desktop/ui/src/lib/localImagePath.ts
  - crates/desktop/ui/src/lib/documentShape.ts
  - crates/desktop/ui/src/lib/responseStreaming.ts
  - crates/desktop/ui/src/lib/resumableHighlight.ts
  - crates/desktop/ui/src/lib/oneByteString.ts
tests:
  - crates/desktop/ui/src/lib/assistantTrace.test.ts
  - crates/desktop/ui/src/lib/subagents.test.ts
  - crates/desktop/ui/e2e/subagents.spec.ts
  - crates/desktop/ui/src/lib/markdownBlocks.test.ts
  - crates/desktop/ui/src/lib/markdownStreaming.test.ts
  - crates/desktop/ui/src/lib/markdownExport.test.ts
  - crates/desktop/ui/src/lib/codeLanguage.test.ts
  - crates/desktop/ui/src/lib/externalLinks.test.ts
  - crates/desktop/ui/src/lib/imageSrc.test.ts
  - crates/desktop/ui/src/lib/chatImageSrc.test.ts
  - crates/desktop/ui/src/lib/localImagePath.test.ts
  - crates/desktop/ui/src/lib/documentShape.test.ts
  - crates/desktop/ui/src/lib/resumableHighlight.test.ts
  - crates/desktop/ui/src/lib/oneByteString.test.ts
  - crates/desktop/ui/src/lib/katexSecurity.test.mjs
  - crates/desktop/ui/e2e/streaming-performance.spec.ts
  - crates/desktop/ui/e2e/code-highlighting.spec.ts
  - crates/desktop/ui/e2e/markdown-diagrams.spec.ts
  - crates/desktop/ui/e2e/fixtures/streaming.html
verify:
  - npm run ui:test
  - npm run ui:e2e -- streaming-performance
  - npm run ui:e2e -- code-highlighting
  - npm run ui:e2e -- markdown-diagrams
  - npm run ui:e2e -- subagents
  - cargo test -p zest-desktop --lib -- markdown_export_tests
---

# Message rendering

## Behavior
- The work behind an answer renders above it as one shadcn `Trace`, in the order it happened (AI Elements patterns): each thinking block is a Reasoning line, each tool call a Tool line, two or more plain calls in a row one Chain of Thought line, and provider activity (Claude Code, Codex, Cursor) Tool lines labelled with the provider. A running line shimmers; a failed one is red; a pending approval is a dimmed "Waiting for approval" line while its card stays above the composer. Truncated titles show their full text on hover.
- Work handed to another agent is a framed Subagent line (bot icon, model or provider as a badge, duration) that never folds into a run: `delegate_external` ("Ran Codex as a subagent", with the worker's report and an "Open changes" button), `delegate_feature` ("Worker running: <card title>", read live from the board, with the card's approval, worker and reviewer attempts, checks and findings nested under it, plus "Open in Delegation"), and a Claude Code `Agent`/`Task` row ("Running subagent: <description>", with the activities whose `parentId` names it nested under it, or its last reported step). A running subagent opens itself and folds when it settles.
- Explicitly closed `zest-html` fences and completed native HTML tool results show interactive artifact cards; ordinary HTML code stays inert. See [Interactive HTML artifacts](html-artifacts.md) for isolation and verification.
- Assistant text renders as GitHub-flavoured Markdown (tables, lists, headings). User bubbles are plain text with bare URLs linkified (`LinkifyText`, trailing punctuation trimmed).
- Only `http(s)` links become anchors; any other scheme renders as plain text. A paragraph that is a single link renders as a link-preview card with its host and an Open button.
- Clicking an `http(s)` link opens the system browser; the webview never navigates. Alt-click and `download` anchors are left alone.
- Fenced code shows a language chip and a "Copy code" button. Text is readable immediately while streaming and gains colour shortly after. Every language with a grammar in `codeBlockLanguages` is coloured, including aliases such as `rb`, `kt`, `ps1` and `dockerfile`; a tag with no grammar keeps its label and renders uncoloured.
- ` ```mermaid ` fences render as a diagram once the fence has settled; while streaming or when invalid they stay a copyable code block. Diagrams and images expand to a zoomable overlay (+/-/0, Esc closes).
- Images: `http(s)` URLs, raster `data:` URLs, and absolute local image paths (served through Tauri's asset protocol). Local paths written in prose (bare path, code span, `file://`) get an image hoisted above the line. Backtick and tilde fenced contents are never rewritten, including image-like path strings and Markdown image syntax inside HTML scripts. Broken images are hidden.
- Settled answers show "Copy Markdown" (raw source) and "Save as <name>.md" (native save dialog). Neither shows while the message streams.
- Block streaming (Settings toggle, default on, key `zest.responseBlockStreaming.v1`) reveals an answer block by block instead of re-rendering a growing tail.
- For turns produced by a command (for example Plan mode), an answer is framed as a document card once it is 400+ chars or contains a heading or ordered list (`looksLikeDocument`).

## How it works
- `ChatScreen.tsx` `ChatMessageRow` (and each split pane) -> `AssistantTrace.tsx`: `buildTrace` (`assistantTrace.ts`) resolves the message's `steps` to its parts (older threads: thinking, tools, then activity; a part no step names is appended, never dropped), nests activities under their `parentId`, and folds runs of at least `COLLAPSE_THRESHOLD` plain calls. Lines are `ThinkingReasoning`, `ToolLine` / `ToolRunGroup`, and `ActivityLine` / `ActivityGroup`, all on `components/ui/trace.tsx`; tool output sits in the shadcn `components/ui/code-block.tsx` at `size="sm"`. Subagent items go to `SubagentLine.tsx`: `isSubagentTool` / `parseAgentTitle` pick them out, and `subagents.ts` turns a `DelegationJob` from `App`'s board state into the line's status (`jobProgress`) and nested steps (`jobSteps`).
- `ChatScreen.tsx` `ChatMessageRow` -> `Markdown.tsx` (`react-markdown` + `remark-gfm`). `hoistLocalImages` (`localImagePath.ts`) rewrites prose only, preserving fenced bytes until the same marker closes with at least the opening length and no suffix other than whitespace. Then `splitRenderableBlocks` (`markdownBlocks.ts`) cuts top-level blocks keyed by index; each `Block` is memoized. With block streaming on, the incomplete tail is withheld and fences reach `CodeBlock` with `streaming=false`; with it off, the tail renders and `streaming` is forwarded.
- `pre` -> `CodeBlock.tsx` (`normalizeLang`/`languageLabel` from `codeLanguage.ts`, which maps a few spellings and otherwise passes the fence tag through) -> ReUI `code-block.tsx`: `useDeferredValue` source, plain lines as the floor, `highlightCode` (`code-block-highlight.tsx`, shiki core on the JavaScript regex engine, grammars lazy-loaded from a static import map); a growing fence resumes the grammar after its last complete line (`createResumableHighlight` in `resumableHighlight.ts`) instead of re-tokenizing the whole block, and `toOneByteIfLatin1` (`oneByteString.ts`) hands shiki a one-byte string when it can, which keeps its regexes off V8's slower two-byte path debounced 150 ms while streaming and immediate once complete. Highlight results are tagged with their source and spec so a swapped document never shows stale colours. The block sticks to the bottom while streaming and announces completion to screen readers.
- `MermaidBlock.tsx`: dynamic `import("mermaid")`, `securityLevel: "strict"`, theme follows the app appearance, SVG inserted via `markTrustedHtml` (`safeHtml.ts`, a provenance brand, not a sanitizer). The browser regression covers math labels and the expanded diagram. The root dependency override selects KaTeX 0.18.2 for Mermaid without downgrading Mermaid. The security regression resolves Mermaid's actual math renderer and proves inherited `trust` cannot enable HTML links, while explicit trusted links and ordinary math still render.
- `ZoomableImage.tsx` -> `resolveChatImageSrc` (`chatImageSrc.ts`) -> `safeImageSrc` (`imageSrc.ts`) or `parseLocalImagePath` + `convertFileSrc`. `ImageLightbox` is shared with composer attachment chips.
- Links: `safeHttpUrl` and `installExternalLinkHandling` (capture-phase click/auxclick) in `externalLinks.ts` -> `openExternalUrl`.
- `MarkdownActions.tsx` -> `suggestedMarkdownFilename` (`markdownExport.ts`, first heading, Windows-reserved names prefixed, 120 chars) -> Tauri `save_markdown` (`crates/desktop/src/lib.rs`), which enforces `.md` and remembers the directory.

## Verify
- `npm run ui:e2e -- streaming-performance` drives the real `CodeBlock` through `e2e/fixtures/streaming.html`: final text, growing last line, language swap, copy, unmount mid-highlight, typing responsiveness. `ZEST_PERF=1 ... --workers=1` adds the opt-in benchmark (plans/005-streaming-highlight-results.md).
- `npm run ui:e2e -- code-highlighting` sends one fence per language through the fixture chat and asserts each block is labelled and coloured.
- In `?fixture=1`, send a message containing a fence, a Mermaid block, a table and a link to check rendering end to end.

## Pitfalls
- The block splitter must stay conservative: over-splitting changes meaning (a list split in two, an indented paragraph turned into code). Blocks must be byte-stable under append so index keys stay valid.
- `looksLikeDocument` must stay monotonic; a predicate that flips back would wrap and unwrap the card mid-stream.
- The grammar list lives only in `codeBlockLanguages` (`code-block-highlight.tsx`). `codeLanguage.ts` must not keep its own allowlist: it once did, and silently turned ruby, kotlin, swift, php and others into plain text. Add a language by adding a grammar line there.
- Do not hand `transformers` a new array per render; it re-tokenizes the whole block per chunk (dev warning in `code-block.tsx`).
- The overlays use `react-dom` `createPortal`; the UI README's ban is on Base UI Menu/Portal.
- Two code blocks on purpose: Markdown fences use the ReUI block (highlighting that resumes while it streams); tool output in Trace lines uses the plain shadcn `ui/code-block`.
- Trace rows clip (`overflow: hidden`) for their open/close animation, so hover text is `TraceHoverTitle`: in the tree (no portal, which the WebView drops) but `position: fixed` at the title's rectangle.
- A Claude Code subagent's own tool calls only nest once core sends them as activities with a `parent_id` (the `parent_tool_use_id` in `claude_stream.rs`). Until then the wire carries none and the line shows the progress core appends to the `Agent` title.
- The shadcn CLI writes new components with `import { cn } from "cn"` and adds the `cn` npm package; point them at `@/lib/utils` instead, as the existing components do.
