import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { displayToolSummary, wasInterrupted } from "./toolDisplay.ts";

describe("displayToolSummary", () => {
  it("drops a note that is only the interrupt label", () => {
    assert.equal(displayToolSummary("interrupted"), undefined);
    assert.equal(displayToolSummary("approval interrupted"), undefined);
    assert.equal(displayToolSummary("tool interrupted"), undefined);
  });

  it("keeps the original summary when a note was appended", () => {
    assert.equal(displayToolSummary("src/App.jsx (interrupted)"), "src/App.jsx");
  });

  it("leaves a real result alone", () => {
    assert.equal(displayToolSummary("12 files"), "12 files");
  });
});

describe("wasInterrupted", () => {
  it("recognizes the notes the reducer writes", () => {
    assert.equal(wasInterrupted("interrupted"), true);
    assert.equal(wasInterrupted("list (interrupted)"), true);
    assert.equal(wasInterrupted("12 files"), false);
  });
});
