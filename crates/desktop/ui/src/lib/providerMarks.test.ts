import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { hasProviderMark, providerMark } from "./providerMarks.ts";

describe("provider marks", () => {
  it("recognises the providers Zest ships with", () => {
    assert.equal(providerMark("codex"), "codex");
    assert.equal(providerMark("claude"), "claude");
    assert.equal(providerMark("antigravity"), "gemini");
    assert.equal(providerMark("cursor"), "cursor");
    assert.equal(providerMark("cursor-work"), "cursor");
  });

  it("recognises providers people configure by hand", () => {
    // `deepseek` is in the shipped zest.toml.example; `anthropic` is the
    // documented native provider id.
    assert.equal(providerMark("deepseek"), "deepseek");
    assert.equal(providerMark("anthropic"), "claude");
    assert.equal(providerMark("openai"), "codex");
  });

  it("matches regardless of case or a suffix", () => {
    // Nothing stops someone naming a provider `anthropic-work`, and an id that
    // is obviously Anthropic should not drop to the generic mark.
    assert.equal(providerMark("Codex"), "codex");
    assert.equal(providerMark("codex-chatgpt"), "codex");
    assert.equal(providerMark("anthropic-work"), "claude");
    assert.equal(providerMark("  deepseek  "), "deepseek");
  });

  it("falls back for anything unmapped", () => {
    // The expected case for a local model, not a failure.
    for (const id of ["ollama", "lmstudio", "my-gateway", "", undefined, null]) {
      assert.equal(providerMark(id), "generic", String(id));
      assert.equal(hasProviderMark(id), false, String(id));
    }
  });
});
