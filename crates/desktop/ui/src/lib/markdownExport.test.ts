import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  commandMarkdownFilename,
  safeMarkdownFilename,
  suggestedMarkdownFilename,
} from "./markdownExport.ts";

describe("Markdown export filenames", () => {
  it("uses the first heading", () => {
    assert.equal(
      suggestedMarkdownFilename("# Release roadmap\n\nDetails"),
      "Release roadmap.md"
    );
  });

  it("supports an empty heading and falls back for plain replies", () => {
    assert.equal(suggestedMarkdownFilename("##    \n\nAnswer"), "response.md");
    assert.equal(suggestedMarkdownFilename("Just an answer."), "response.md");
  });

  it("sanitizes unsafe characters and preserves the Markdown extension", () => {
    assert.equal(
      safeMarkdownFilename(' plan: <draft> / "v1"?.txt '),
      "plan- -draft- - -v1--.txt.md"
    );
    assert.equal(commandMarkdownFilename("plan"), "plan.md");
    assert.equal(commandMarkdownFilename("plan.md"), "plan.md");
  });

  it("guards Windows device names", () => {
    assert.equal(safeMarkdownFilename("CON"), "_CON.md");
    assert.equal(safeMarkdownFilename("..."), "response.md");
  });
});
