import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  isLongTurn,
  LONG_TURN_NOTIFICATION_MS,
  notificationFingerprint,
} from "./notificationPolicy.ts";

describe("notification policy", () => {
  it("only marks turns at or above the long-turn threshold", () => {
    assert.equal(isLongTurn(LONG_TURN_NOTIFICATION_MS - 1), false);
    assert.equal(isLongTurn(LONG_TURN_NOTIFICATION_MS), true);
    assert.equal(isLongTurn(LONG_TURN_NOTIFICATION_MS + 1), true);
  });

  it("groups only notifications with the same visible content", () => {
    const first = notificationFingerprint(
      "error",
      "Could not allow tool",
      "That request expired."
    );
    assert.equal(
      notificationFingerprint("error", "Could not allow tool", "That request expired."),
      first
    );
    assert.notEqual(
      notificationFingerprint("warning", "Could not allow tool", "That request expired."),
      first
    );
    assert.notEqual(
      notificationFingerprint("error", "Could not allow tool", "Try again."),
      first
    );
  });
});
