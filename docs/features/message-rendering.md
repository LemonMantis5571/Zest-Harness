---
title: Message rendering
summary: Assistant answers render as Markdown with highlighted code, Mermaid diagrams, zoomable images, safe links, and copy/save actions, even while they stream.
paths:
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
  - cargo test -p zest-desktop --lib -- markdown_export_tests
---

# Message rendering

## Behavior
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
