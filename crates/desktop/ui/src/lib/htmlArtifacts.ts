export type HtmlDocument = { title: string; html: string };
export type PreparedHtmlView = { token: string; url: string };

export const MAX_HTML_BYTES = 512 * 1024;
export const HTML_CSP = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; connect-src 'none'; frame-src 'none'; worker-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; sandbox allow-scripts";

/** Validate untrusted wire data before handing it to a viewer or exporter. */
export function parseHtmlDocument(value: unknown): HtmlDocument {
  if (!value || typeof value !== "object" || !("title" in value) || !("html" in value) ||
      typeof value.title !== "string" || typeof value.html !== "string") {
    throw new Error("Expected an HTML document with title and html.");
  }
  if (!value.html.trim() || value.html.includes("\0")) {
    throw new Error("HTML must be nonempty and contain no NUL characters.");
  }
  const title = value.title.trim();
  if (!title || [...title].length > 120 || [...title].some((character) => {
    const code = character.charCodeAt(0);
    return code < 32 || (code >= 127 && code <= 159);
  })) throw new Error("HTML title must be 1–120 characters without control characters.");
  if (new TextEncoder().encode(value.html).length > MAX_HTML_BYTES) {
    throw new Error("HTML exceeds 512 KiB.");
  }
  return { title, html: value.html };
}

export function htmlDocumentFromMetadata(value: unknown): HtmlDocument | null {
  if (!value || typeof value !== "object" || !("kind" in value) || value.kind !== "html_document") return null;
  try {
    return parseHtmlDocument(value);
  } catch {
    return null;
  }
}

/** Only a standalone, explicitly closed zest-html fence opts into a card. */
export function htmlDocumentFromFence(text: string): HtmlDocument | null {
  const lines = text.trimEnd().split("\n");
  const opening = /^ {0,3}(`{3,}|~{3,})zest-html(?:[ \t]+([^`]*))?[ \t]*$/.exec((lines[0] ?? "").replace(/\r$/, ""));
  if (!opening || lines.length < 3) return null;
  const fence = opening[1];
  const close = new RegExp(`^ {0,3}${fence[0]}{${fence.length},}[ \\t]*$`);
  // An earlier close makes the remainder prose, not part of this document.
  const closingIndex = lines.findIndex((line, index) => index > 0 && close.test(line.replace(/\r$/, "")));
  if (closingIndex !== lines.length - 1) return null;
  // Exclude the closing fence's separator CR, preserving line endings inside HTML.
  const html = lines.slice(1, -1).join("\n").replace(/\r$/, "");
  try {
    return parseHtmlDocument({ title: opening[2]?.trim() || "HTML preview", html });
  } catch {
    return null;
  }
}

function escapeAttribute(value: string): string {
  return value.replaceAll("&", "&amp;").replaceAll('"', "&quot;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");
}

/** Policy and bootstrap precede every untrusted byte. HTTP CSP is authoritative. */
export function isolatedHtml(document: HtmlDocument): string {
  const valid = parseHtmlDocument(document);
  const rtcBootstrap = `<script>for(const key of ['RTCPeerConnection','webkitRTCPeerConnection','RTCDataChannel']){try{Object.defineProperty(globalThis,key,{value:undefined,writable:false,configurable:false});}catch{}}</script>`;
  // Browsers only support sandbox as a response header or iframe attribute.
  const exportPolicy = HTML_CSP.replace("; sandbox allow-scripts", "");
  return `<!doctype html><meta charset="utf-8"><meta name="referrer" content="no-referrer"><meta http-equiv="Content-Security-Policy" content="${escapeAttribute(exportPolicy)}"><title>${escapeAttribute(valid.title)}</title>${rtcBootstrap}${valid.html}`;
}

export function htmlFilename(title: string): string {
  const stem = Array.from(title, (character) => character.charCodeAt(0) < 32 || /[<>:"/\\|?*]/.test(character) ? "-" : character)
    .join("").replace(/[ .]+$/g, "").slice(0, 120) || "document";
  return `${/^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i.test(stem) ? "_" : ""}${stem}.html`;
}
