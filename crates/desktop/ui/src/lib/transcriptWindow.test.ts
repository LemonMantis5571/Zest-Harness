import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  TRANSCRIPT_REVEAL_STEP,
  TRANSCRIPT_WINDOW_LIMIT,
  TRANSCRIPT_WINDOW_SIZE,
  clampTranscriptStart,
  initialTranscriptStart,
  revealEarlierTranscriptStart,
  shouldTrimTranscript,
  transcriptStartForTarget,
} from "./transcriptWindow.ts";

describe("transcript window", () => {
  it("mounts the whole short transcript", () => {
    assert.equal(initialTranscriptStart(TRANSCRIPT_WINDOW_SIZE), 0);
  });

  it("mounts only the recent tail of a long transcript", () => {
    assert.equal(initialTranscriptStart(1_000), 1_000 - TRANSCRIPT_WINDOW_SIZE);
  });

  it("reveals history in bounded chunks", () => {
    assert.equal(
      revealEarlierTranscriptStart(500),
      500 - TRANSCRIPT_REVEAL_STEP
    );
    assert.equal(revealEarlierTranscriptStart(20), 0);
  });

  it("keeps at least a normal tail window after rewind", () => {
    assert.equal(clampTranscriptStart(120, 900), 120 - TRANSCRIPT_WINDOW_SIZE);
    assert.equal(clampTranscriptStart(120, 0), 0);
  });

  it("expands before jumping to a hidden checkpoint", () => {
    assert.equal(transcriptStartForTarget(920, 120), 116);
    assert.equal(transcriptStartForTarget(920, 950), 920);
    assert.equal(transcriptStartForTarget(920, -1), 920);
  });

  it("trims expanded history only after returning to the end", () => {
    const count = 500;
    const expandedStart = count - TRANSCRIPT_WINDOW_LIMIT - 1;
    assert.equal(shouldTrimTranscript(count, expandedStart, false), false);
    assert.equal(shouldTrimTranscript(count, expandedStart, true), true);
    assert.equal(
      shouldTrimTranscript(count, count - TRANSCRIPT_WINDOW_LIMIT, true),
      false
    );
  });
});
