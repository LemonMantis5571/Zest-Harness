import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { languageLabel, normalizeLang } from "./codeLanguage.ts";

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
