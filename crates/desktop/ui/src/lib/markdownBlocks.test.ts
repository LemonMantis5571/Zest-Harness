import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { splitBlocks } from "./markdownBlocks.ts";

const texts = (md: string) => splitBlocks(md).map((b) => b.text);

describe("splitBlocks — correctness", () => {
  it("splits plain paragraphs", () => {
    assert.deepEqual(texts("one\n\ntwo\n\nthree"), ["one", "two", "three"]);
  });

  it("never splits inside a fenced code block", () => {
    const md = "before\n\n```ts\nconst a = 1;\n\nconst b = 2;\n```\n\nafter";
    const out = texts(md);
    assert.equal(out.length, 3);
    assert.ok(out[1].startsWith("```ts"), out[1]);
    assert.ok(out[1].includes("const b = 2;"), "blank line split the fence");
    assert.equal(out[2], "after");
  });

  it("only closes a fence with a matching marker", () => {
    // A ~~~ line must not close a ``` fence.
    const md = "```\n~~~\nstill code\n```\n\nafter";
    const out = texts(md);
    assert.equal(out.length, 2);
    assert.ok(out[0].includes("still code"));
    assert.equal(out[1], "after");
  });

  it("keeps a loose list together", () => {
    // Split, this would render as three separate <ul>s.
    const md = "- one\n\n- two\n\n- three";
    assert.equal(texts(md).length, 1);
  });

  it("keeps a multi-paragraph list item together", () => {
    // Orphaned, the indented paragraph becomes a code block.
    const md = "- item\n\n  second paragraph of the item";
    assert.equal(texts(md).length, 1);
  });

  it("ends a list when an unindented paragraph follows", () => {
    const md = "- one\n- two\n\nA new paragraph.";
    assert.deepEqual(texts(md), ["- one\n- two", "A new paragraph."]);
  });

  it("gives a fence its own block", () => {
    // So settled prose before the fence is not re-parsed while code streams.
    const md = "intro\n```js\nx\n```";
    const out = texts(md);
    assert.equal(out.length, 2);
    assert.equal(out[0], "intro");
  });

  it("handles empty and whitespace-only input", () => {
    assert.deepEqual(splitBlocks(""), []);
    assert.deepEqual(splitBlocks("\n\n  \n"), []);
  });

  it("loses no content", () => {
    const md = "# Title\n\npara\n\n- a\n- b\n\n```\ncode\n```\n\nend";
    const joined = texts(md).join("\n").replace(/\s+/g, " ").trim();
    const original = md.replace(/\s+/g, " ").trim();
    assert.equal(joined, original);
  });
});

describe("splitBlocks — append-only stability", () => {
  /**
   * The property memoization depends on: growing the text must not disturb
   * any block except the last. If it does, React remounts settled blocks and
   * the whole optimisation inverts into extra work.
   */
  function assertStable(full: string, label: string) {
    let previous = splitBlocks("");
    for (let i = 1; i <= full.length; i += 1) {
      const blocks = splitBlocks(full.slice(0, i));
      // Every block before the last must match the earlier run exactly.
      const settled = blocks.slice(0, -1);
      for (let b = 0; b < settled.length && b < previous.length - 1; b += 1) {
        assert.equal(
          settled[b].text,
          previous[b].text,
          `${label}: block ${b} changed at length ${i}`
        );
        assert.equal(settled[b].key, b, `${label}: key ${b} is not its index`);
      }
      previous = blocks;
    }
  }

  it("is stable for prose with a list and a fence", () => {
    assertStable(
      "# Plan\n\nFirst para.\n\n- one\n- two\n\n```ts\nconst a = 1;\n```\n\nDone.",
      "mixed"
    );
  });

  it("is stable for a loose list", () => {
    assertStable("- one\n\n- two\n\n- three\n\nafter", "loose list");
  });

  it("is stable while a fence is still open", () => {
    // The open fence block legitimately grows; everything before it must not.
    const full = "intro\n\n```\nline one\nline two\n";
    const before = splitBlocks(full.slice(0, 12));
    const after = splitBlocks(full);
    assert.equal(before[0].text, "intro");
    assert.equal(after[0].text, "intro");
  });
});

describe("splitBlocks — the point of it", () => {
  it("keeps settled blocks out of the re-parse as text grows", () => {
    // A frame only needs to re-render blocks whose text changed. With one
    // string that is always the whole document; with blocks it is the tail.
    const settled = "para one\n\npara two\n\npara three\n\n";
    const before = splitBlocks(settled + "tai");
    const after = splitBlocks(settled + "tail");

    const changed = after.filter(
      (b, i) => before[i]?.text !== b.text
    );
    assert.equal(changed.length, 1, "only the trailing block should change");
    assert.equal(changed[0].text, "tail");
  });
});
