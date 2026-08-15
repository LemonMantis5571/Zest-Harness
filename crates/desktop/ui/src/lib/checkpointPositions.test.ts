import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { resolveCheckpointMarkerPositions } from "./checkpointPositions.ts";

describe("checkpoint marker positions", () => {
  it("separates markers that are all pinned to the top edge", () => {
    const positions = resolveCheckpointMarkerPositions([0, 2], 160);

    assert.deepEqual(positions, [8, 40]);
  });

  it("separates markers pinned to the bottom without moving visible anchors", () => {
    const positions = resolveCheckpointMarkerPositions([112, 300], 160);

    assert.deepEqual(positions, [100, 132]);
  });

  it("keeps the original checkpoint order in the returned array", () => {
    const positions = resolveCheckpointMarkerPositions([300, 0], 160);

    assert.deepEqual(positions, [132, 8]);
  });
});
