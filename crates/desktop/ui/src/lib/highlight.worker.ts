/// <reference lib="webworker" />
/**
 * Shiki, off the main thread.
 *
 * Highlighting a single block costs upwards of 100 ms, and a chat can be
 * highlighting several while text is still streaming. On the main thread that
 * is dropped frames; here it is not the main thread's problem at all.
 *
 * The worker owns the whole highlighter — grammars are statically imported in
 * `highlight-core`, so nothing is fetched at runtime and WebView2 needs no
 * WASM. Requests carry an `id` and answers carry it back, because the queue on
 * the other side cancels superseded work and must be able to ignore a late
 * reply for a block that has already moved on.
 */
import { highlightToHtml } from "./highlight-core.ts";
import type { HighlightRequest, HighlightResponse } from "./highlight-protocol.ts";

const ctx = self as unknown as DedicatedWorkerGlobalScope;

ctx.addEventListener("message", (event: MessageEvent<HighlightRequest>) => {
  const { id, code, lang } = event.data;
  highlightToHtml(code, lang)
    .then((html) => {
      const done: HighlightResponse = { id, ok: true, html };
      ctx.postMessage(done);
    })
    .catch((err: unknown) => {
      // The caller falls back to plain text; it only needs to know it failed.
      const failed: HighlightResponse = {
        id,
        ok: false,
        error: err instanceof Error ? err.message : String(err),
      };
      ctx.postMessage(failed);
    });
});
