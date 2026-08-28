import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { nowPlayingButtonVisible, nowPlayingPluginState } from "./nowPlayingPluginState.ts";
import type { PluginView } from "./types.ts";

const plugin: PluginView = {
  id: "now-playing",
  name: "Now Playing",
  description: "See and control your music.",
  enabled: true,
  available: true,
  detail: "Ready",
};

describe("now playing plugin states", () => {
  it("keeps the entry in a checking state before discovery completes", () => {
    assert.equal(nowPlayingPluginState(false, null), "checking");
  });

  it("keeps a missing plugin discoverable", () => {
    assert.equal(nowPlayingPluginState(true, null), "missing");
  });

  it("separates an installed but unavailable plugin from a missing one", () => {
    assert.equal(
      nowPlayingPluginState(true, { ...plugin, available: false, detail: "Not ready" }),
      "unavailable"
    );
  });

  it("keeps an available plugin visibly off until the user turns it on", () => {
    assert.equal(nowPlayingPluginState(true, { ...plugin, enabled: false }), "disabled");
  });

  it("moves from the missing state to ready after refresh finds the add-on", () => {
    assert.equal(nowPlayingPluginState(true, null), "missing");
    assert.equal(nowPlayingPluginState(true, plugin), "ready");
  });
});

describe("now playing button visibility", () => {
  /** A fresh install has no add-on, so the topbar carries no dead control. */
  it("stays out of the topbar when the add-on is not installed", () => {
    assert.equal(nowPlayingButtonVisible(nowPlayingPluginState(true, null)), false);
  });

  it("stays hidden while discovery is still running, so it never flashes", () => {
    assert.equal(nowPlayingButtonVisible(nowPlayingPluginState(false, null)), false);
  });

  it("appears once the add-on is installed and turned on", () => {
    assert.equal(nowPlayingButtonVisible(nowPlayingPluginState(true, plugin)), true);
  });

  it("stays hidden while the add-on is off or broken", () => {
    assert.equal(
      nowPlayingButtonVisible(nowPlayingPluginState(true, { ...plugin, enabled: false })),
      false
    );
    assert.equal(
      nowPlayingButtonVisible(nowPlayingPluginState(true, { ...plugin, available: false })),
      false
    );
  });
});
