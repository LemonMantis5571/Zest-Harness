import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { shouldPreserveNativeContextMenu } from "./contextMenu.ts";

type FakeNode = {
  tagName?: string;
  contentEditable?: string;
  parentElement?: FakeNode | null;
};

function node({
  tagName = "div",
  contentEditable,
  parentElement = null,
}: FakeNode = {}): FakeNode {
  return {
    tagName,
    parentElement,
    getAttribute(name: string) {
      return name === "contenteditable" ? contentEditable ?? null : null;
    },
  } as FakeNode;
}

describe("native context menu guard", () => {
  it("keeps edit menus for native form controls", () => {
    for (const tagName of ["input", "textarea", "select"]) {
      assert.equal(shouldPreserveNativeContextMenu(node({ tagName })), true);
    }
  });

  it("keeps the menu for descendants of editable content", () => {
    const editor = node({ contentEditable: "plaintext-only" });
    assert.equal(
      shouldPreserveNativeContextMenu(node({ parentElement: editor })),
      true
    );
  });

  it("recognizes every editable contenteditable value", () => {
    for (const contentEditable of ["", "true", "plaintext-only"]) {
      assert.equal(
        shouldPreserveNativeContextMenu(node({ contentEditable })),
        true
      );
    }
  });

  it("does not inherit through an explicitly non-editable region", () => {
    const editor = node({ contentEditable: "true" });
    const readOnly = node({ contentEditable: "false", parentElement: editor });
    assert.equal(shouldPreserveNativeContextMenu(readOnly), false);
  });

  it("blocks the page menu for ordinary transcript content and links", () => {
    const link = node({ tagName: "a" });
    const transcript = node({ parentElement: link });
    assert.equal(shouldPreserveNativeContextMenu(transcript), false);
  });
});
