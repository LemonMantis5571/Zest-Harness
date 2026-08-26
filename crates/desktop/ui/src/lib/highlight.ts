/**
 * Main-thread client for the Shiki worker.
 *
 * Highlighting is expensive enough — over 100 ms for one block — that doing it
 * here drops frames while text is still streaming. The work happens in
 * `highlight.worker.ts`; this file is the queue in front of it.
 *
 * The queue is a **latest-value cache keyed per block**, not a backlog: a
 * streaming code block asks to be highlighted every time it settles, and only
 * the newest ask is worth answering. Superseding an in-flight request costs
 * nothing, because replies are matched by id and a dropped id is simply
 * ignored when it arrives.
 */
import { normalizeLang } from "./highlight-core.ts";
import type { HighlightRequest, HighlightResponse } from "./highlight-protocol.ts";

export { languageLabel, normalizeLang } from "./highlight-core.ts";

type Pending = {
  resolve: (html: string) => void;
  reject: (err: Error) => void;
};

let worker: Worker | null = null;
let workerFailed = false;
let nextId = 1;
/** In-flight requests by id. A superseded id is deleted, so its reply is dropped. */
const pending = new Map<number, Pending>();
/** Newest request id per key, so an older reply for a block cannot win. */
const latestByKey = new Map<string, number>();

function failAll(reason: string) {
  for (const slot of pending.values()) slot.reject(new Error(reason));
  pending.clear();
  latestByKey.clear();
}

function ensureWorker(): Worker | null {
  if (worker) return worker;
  // One failure is enough — retrying a CSP-blocked constructor every block
  // would throw on every code fence in the transcript.
  if (workerFailed) return null;
  try {
    const next = new Worker(new URL("./highlight.worker.ts", import.meta.url), {
      type: "module",
    });
    next.addEventListener(
      "message",
      (event: MessageEvent<HighlightResponse>) => {
        const reply = event.data;
        const slot = pending.get(reply.id);
        // No slot means the request was superseded while in flight. Dropping
        // it is the entire point of keying by block.
        if (!slot) return;
        pending.delete(reply.id);
        if (reply.ok) slot.resolve(reply.html);
        else slot.reject(new Error(reply.error));
      }
    );
    next.addEventListener("error", () => {
      // A dead worker must not leave callers awaiting forever.
      failAll("highlight worker failed");
      worker = null;
      workerFailed = true;
    });
    worker = next;
  } catch {
    // Blocked by CSP or unsupported — callers keep their plain-text fallback.
    workerFailed = true;
    worker = null;
  }
  return worker;
}

/**
 * Highlight `code`, superseding any pending request for the same `key`.
 *
 * Rejects when the worker is unavailable. Every caller already renders plain
 * monospace until an answer arrives, so that degrades rather than breaks.
 */
export function highlightCode(
  code: string,
  langHint?: string | null,
  key?: string
): Promise<string> {
  const active = ensureWorker();
  if (!active) return Promise.reject(new Error("highlight worker unavailable"));

  const id = nextId++;
  const slotKey = key ?? `anon-${id}`;

  const previous = latestByKey.get(slotKey);
  if (previous !== undefined) {
    const stale = pending.get(previous);
    if (stale) {
      pending.delete(previous);
      // Settle it, so nobody awaits a promise that can never resolve.
      stale.reject(new Error("superseded"));
    }
  }
  latestByKey.set(slotKey, id);

  const request: HighlightRequest = {
    id,
    code,
    lang: normalizeLang(langHint),
  };

  return new Promise<string>((resolve, reject) => {
    pending.set(id, { resolve, reject });
    active.postMessage(request);
  });
}

/** Drop a block's slot when its component unmounts. */
export function releaseHighlight(key: string): void {
  const id = latestByKey.get(key);
  if (id === undefined) return;
  pending.get(id)?.reject(new Error("released"));
  pending.delete(id);
  latestByKey.delete(key);
}
