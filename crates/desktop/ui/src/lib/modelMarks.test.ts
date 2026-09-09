import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { modelMark } from "./modelMarks.ts";

describe("model marks", () => {
  it("gives the named GPT variants distinct marks", () => {
    assert.equal(modelMark("gpt-5.6-sol"), "sol");
    assert.equal(modelMark("gpt-5.6-terra"), "terra");
    assert.equal(modelMark("gpt-5.6-luna"), "luna");
  });

  it("recognises common model families", () => {
    assert.equal(modelMark("gpt-5.4-mini"), "mini");
    assert.equal(modelMark("codex-mini-latest"), "codex");
    assert.equal(modelMark("cursor-small"), "cursor");
    assert.equal(modelMark("claude-sonnet"), "claude");
    assert.equal(modelMark("gemini-3-pro"), "gemini");
    assert.equal(modelMark("deepseek-v4"), "deepseek");
    assert.equal(modelMark("meta-llama/llama-4"), "llama");
    assert.equal(modelMark("gpt-5.5"), "standard");
  });

  it("normalises ids and falls back for unknown models", () => {
    assert.equal(modelMark(" GPT-5.6-SOL "), "sol");
    for (const id of ["local-model", "", undefined, null]) {
      assert.equal(modelMark(id), "generic", String(id));
    }
  });
});
