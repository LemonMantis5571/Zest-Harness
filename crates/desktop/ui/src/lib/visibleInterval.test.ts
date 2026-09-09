import assert from "node:assert/strict";
import { it } from "node:test";
import { visibleInterval } from "./visibleInterval.ts";

class Visibility extends EventTarget {
  hidden = false;
  change(hidden: boolean) {
    this.hidden = hidden;
    this.dispatchEvent(new Event("visibilitychange"));
  }
}

it("pauses hidden polling, refreshes once on return, and cleans up", (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const target = new Visibility();
  let calls = 0;
  const dispose = visibleInterval(target, () => calls++, 2500);
  assert.equal(calls, 0, "opening caller already requests a snapshot");
  t.mock.timers.tick(2500);
  assert.equal(calls, 1);
  target.change(true);
  t.mock.timers.tick(60_000);
  assert.equal(calls, 1, "a hidden minute starts no inspections");
  target.change(false);
  assert.equal(calls, 2);
  target.change(false);
  assert.equal(calls, 2, "duplicate visibility events do not duplicate timers");
  t.mock.timers.tick(2500);
  assert.equal(calls, 3);
  dispose();
  target.change(true);
  target.change(false);
  t.mock.timers.tick(10_000);
  assert.equal(calls, 3);
});

it("starts hidden without polling and does not revive after disposal", (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const target = new Visibility();
  target.hidden = true;
  let calls = 0;
  const dispose = visibleInterval(target, () => calls++, 2500);
  t.mock.timers.tick(10_000);
  assert.equal(calls, 0);
  target.change(false);
  assert.equal(calls, 1);
  target.change(true);
  dispose();
  target.change(false);
  t.mock.timers.tick(10_000);
  assert.equal(calls, 1);
});
