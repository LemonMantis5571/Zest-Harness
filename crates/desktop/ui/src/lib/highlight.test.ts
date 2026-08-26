import assert from "node:assert/strict";
import { describe, it } from "node:test";

// Targets `highlight-core` rather than `highlight`: the highlighting itself
// lives there, while `highlight.ts` is only the worker queue in front of it —
// and a Worker does not exist in this runtime.
import {
  highlightToHtml,
  languageLabel,
  normalizeLang,
} from "./highlight-core.ts";

describe("normalizeLang", () => {
  it("maps aliases", () => {
    assert.equal(normalizeLang("js"), "javascript");
    assert.equal(normalizeLang("TS"), "typescript");
    assert.equal(normalizeLang("text"), "plaintext");
  });
});

describe("languageLabel", () => {
  it("shortens common langs", () => {
    assert.equal(languageLabel("javascript"), "js");
    assert.equal(languageLabel("plaintext"), "text");
  });
});

describe("highlightToHtml", () => {
  it("emits inline color styles for javascript", async () => {
    const html = await highlightToHtml("const x = 1;\nfunction foo() {}", "js");
    assert.match(html, /style="color:#[0-9A-Fa-f]{6}"/);
    assert.match(html, /const/);
  });

  it("escapes plaintext instead of highlighting it", async () => {
    // Plaintext has no grammar, so it takes the hand-built path — which must
    // still escape, since the result is injected with dangerouslySetInnerHTML.
    const html = await highlightToHtml("<script>alert(1)</script>", "text");
    assert.ok(!html.includes("<script>"), html);
    assert.match(html, /&lt;script&gt;/);
  });
});
