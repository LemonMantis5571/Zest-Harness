import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { heatmapTone } from "./heatmap.ts";

describe("heatmapTone", () => {
  it("keeps missing days at the empty step", () => {
    assert.equal(heatmapTone(null, 10), 0);
  });

  it("keeps a recorded zero off the empty step", () => {
    assert.equal(heatmapTone(0, 10), 1);
  });

  it("treats a window with no peak as idle rather than empty", () => {
    assert.equal(heatmapTone(3, 0), 1);
  });

  it("splits activity into four steps against the busiest day", () => {
    assert.equal(heatmapTone(1, 10), 2);
    assert.equal(heatmapTone(3, 10), 3);
    assert.equal(heatmapTone(6, 10), 4);
    assert.equal(heatmapTone(10, 10), 5);
  });
});
