import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { revealCount } from "./reveal.ts";

/** Frames needed to reveal `total` characters at this pacing. */
function framesToDrain(total: number): number {
  let left = total;
  let frames = 0;
  while (left > 0) {
    left -= revealCount(left);
    frames += 1;
    if (frames > 1000) throw new Error("does not terminate");
  }
  return frames;
}

describe("revealCount", () => {
  it("stays under a second even for a whole answer in one event", () => {
    // The property that matters: the reveal must never fall visibly behind the
    // real stream. 60 frames is ~1s at 60fps.
    for (const pending of [40, 400, 4000, 40000]) {
      const frames = framesToDrain(pending);
      assert.ok(frames <= 60, `${pending} chars took ${frames} frames`);
    }
  });

  it("scales logarithmically, not linearly", () => {
    // A fixed characters-per-frame rate would need 100x the frames for 100x the
    // text, which is what puts a long answer seconds behind.
    const small = framesToDrain(400);
    const huge = framesToDrain(40000);
    assert.ok(huge < small * 4, `${small} → ${huge} frames is close to linear`);
  });

  it("takes long enough to read as motion rather than a jump", () => {
    // The whole point. Landing a burst in one frame is the "text just appears"
    // behaviour this exists to fix.
    assert.ok(framesToDrain(200) >= 6);
  });

  it("always advances, so the buffer cannot stall", () => {
    for (const pending of [1, 2, 3, 7, 100]) {
      assert.ok(revealCount(pending) >= 1);
    }
  });

  it("reveals more when more is waiting", () => {
    assert.ok(revealCount(6000) > revealCount(60));
  });
});
