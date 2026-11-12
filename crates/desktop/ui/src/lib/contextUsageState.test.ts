import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { settleContextReading } from "./contextUsageState.ts";
import type { ContextUsage } from "./types.ts";

const usage = (percentFull: number) => ({ percentFull }) as ContextUsage;
// The desktop rejects with its JSON error envelope as a string.
const busy = JSON.stringify({ code: "busy", message: "this chat is still working" });
const broken = JSON.stringify({ code: "no_session", message: "no active session" });

describe("settleContextReading", () => {
  it("stores a successful reading for its chat", () => {
    assert.deepEqual(settleContextReading(null, "t1", { ok: true, usage: usage(10) }), {
      scope: "t1",
      usage: usage(10),
    });
  });

  it("keeps the last reading while a turn holds the same chat", () => {
    const previous = { scope: "t1", usage: usage(10) };
    assert.equal(settleContextReading(previous, "t1", { ok: false, error: busy }), previous);
  });

  it("does not show another chat's reading", () => {
    const previous = { scope: "t1", usage: usage(10) };
    assert.equal(settleContextReading(previous, "t2", { ok: false, error: busy }), null);
  });

  it("clears on failures other than busy", () => {
    const previous = { scope: "t1", usage: usage(10) };
    assert.equal(settleContextReading(previous, "t1", { ok: false, error: broken }), null);
    assert.equal(settleContextReading(previous, "t1", { ok: false, error: new Error("offline") }), null);
  });
});
