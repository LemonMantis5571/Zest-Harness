import assert from "node:assert/strict";
import { test } from "node:test";
import { createSerialActions } from "./serialActions.ts";

test("pane activation and send cannot interleave, including after a failure", async () => {
  const run = createSerialActions();
  const calls: string[] = [];
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const left = run(async () => { calls.push("activate left"); await gate; calls.push("send left"); throw new Error("offline"); });
  const failed = assert.rejects(left, /offline/);
  const right = run(async () => { calls.push("activate right"); calls.push("send right"); });
  await Promise.resolve();
  assert.deepEqual(calls, ["activate left"]);
  release();
  await Promise.all([failed, right]);
  assert.deepEqual(calls, ["activate left", "send left", "activate right", "send right"]);
});
