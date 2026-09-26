import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { languageLabel, normalizeLang } from "./codeLanguage.ts";

describe("normalizeLang", () => {
  it("maps aliases", () => {
    assert.equal(normalizeLang("js"), "javascript");
    assert.equal(normalizeLang("TS"), "typescript");
    assert.equal(normalizeLang("text"), "plaintext");
    assert.equal(normalizeLang("kt"), "kotlin");
    assert.equal(normalizeLang("pwsh"), "powershell");
  });

  it("passes other fence tags through for the highlighter to resolve", () => {
    // A second allowlist here once turned all of these into plain text.
    for (const tag of ["ruby", "rb", "kotlin", "swift", "php", "vue", "graphql", "dockerfile", "powershell", "c++", "c#"]) {
      assert.equal(normalizeLang(tag), tag);
    }
    assert.equal(normalizeLang(" Ruby "), "ruby");
  });

  it("falls back to plain text for empty or unusable tags", () => {
    assert.equal(normalizeLang(undefined), "plaintext");
    assert.equal(normalizeLang(""), "plaintext");
    assert.equal(normalizeLang("has space"), "plaintext");
    assert.equal(normalizeLang("<script>"), "plaintext");
    assert.equal(normalizeLang("x".repeat(40)), "plaintext");
  });
});

describe("languageLabel", () => {
  it("shortens common langs", () => {
    assert.equal(languageLabel("javascript"), "js");
    assert.equal(languageLabel("plaintext"), "text");
    assert.equal(languageLabel("ruby"), "ruby");
  });
});
