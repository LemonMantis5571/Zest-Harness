import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  createNavigationHistory,
  pushNavigation,
  travelNavigation,
  type NavigationDestination,
} from "./navigationHistory.ts";

const chat: NavigationDestination = { kind: "chat" };
const profile: NavigationDestination = { kind: "profile" };
const usage: NavigationDestination = { kind: "usage" };
const settings: NavigationDestination = { kind: "settings", focusUser: true };

describe("navigation history", () => {
  it("records visits and clears forward history after a new destination", () => {
    let history = pushNavigation(createNavigationHistory(), chat);
    history = pushNavigation(history, profile);
    history = pushNavigation(history, usage);

    const back = travelNavigation(history, -1);
    assert.ok(back);
    assert.deepEqual(back.destination, profile);

    const branched = pushNavigation(back.history, settings);
    assert.deepEqual(branched.back, [chat, profile]);
    assert.deepEqual(branched.current, settings);
    assert.deepEqual(branched.forward, []);
  });

  it("round-trips back and forward without losing settings intent", () => {
    let history = pushNavigation(createNavigationHistory(), chat);
    history = pushNavigation(history, settings);

    const back = travelNavigation(history, -1);
    assert.ok(back);
    assert.deepEqual(back.destination, chat);

    const forward = travelNavigation(back.history, 1);
    assert.ok(forward);
    assert.deepEqual(forward.destination, settings);
    assert.deepEqual(forward.history.current, settings);
  });

  it("does not add a duplicate destination", () => {
    let history = pushNavigation(createNavigationHistory(), chat);
    history = pushNavigation(history, profile);
    const same = pushNavigation(history, profile);
    assert.equal(same, history);
  });
});
