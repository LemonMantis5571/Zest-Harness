import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { looksLikeDocument } from "./documentShape.ts";

describe("looksLikeDocument", () => {
  it("does not frame a short clarifying question", () => {
    assert.equal(looksLikeDocument("Which framework do you want to use?"), false);
    assert.equal(looksLikeDocument(""), false);
    assert.equal(looksLikeDocument("Done — the lint config is already there."), false);
  });

  it("frames anything with a heading, however short", () => {
    assert.equal(looksLikeDocument("## Plan\n\nDo the thing."), true);
    assert.equal(looksLikeDocument("# A"), true);
  });

  it("does not mistake a hashtag or a comment for a heading", () => {
    assert.equal(looksLikeDocument("#1 is the failing case"), false);
    assert.equal(looksLikeDocument("use #![allow(dead_code)] there"), false);
  });

  it("frames ordered steps, including the bold form models actually write", () => {
    assert.equal(looksLikeDocument("1. Fix the lint setup\n2. Add git"), true);
    assert.equal(looksLikeDocument("**1. Fix the broken lint setup** npm run"), true);
    assert.equal(looksLikeDocument("1) Fix it"), true);
  });

  it("frames long prose even with no markup at all", () => {
    assert.equal(looksLikeDocument("word ".repeat(200)), true);
  });

  it("is monotonic: once framed, never unframed", () => {
    // The property that matters. Anything else wraps and unwraps the card while
    // the answer is still streaming.
    const samples = [
      "Confirmed: no .git, no eslint.config.js, no README.\n\n## Plan\n\n1. Fix lint\n2. Add git",
      "Which framework?",
      "Short answer.\n\n1. then a step appears",
      "#not-a-heading then later\n\n# a real one",
      "a".repeat(500),
    ];
    for (const sample of samples) {
      let seenTrue = false;
      for (let i = 0; i <= sample.length; i++) {
        const now = looksLikeDocument(sample.slice(0, i));
        if (now) seenTrue = true;
        else if (seenTrue) {
          assert.fail(
            `unframed at ${i} after being framed earlier: ${JSON.stringify(
              sample.slice(0, i)
            )}`
          );
        }
      }
    }
  });
});
