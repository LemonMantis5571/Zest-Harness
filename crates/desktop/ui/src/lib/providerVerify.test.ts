import assert from "node:assert/strict";
import { beforeEach, describe, it } from "node:test";

import {
  VERIFY_TTL_MS,
  forgetProviderVerify,
  getProviderVerify,
  markProviderVerified,
  markProviderVerifyFailed,
  recentVerifyFailed,
  recentVerifySucceeded,
} from "./providerVerify.ts";

/** Minimal localStorage, since these tests run in node rather than a browser. */
function installStorage(initial: Record<string, string> = {}) {
  const map = new Map(Object.entries(initial));
  const storage = {
    getItem: (k: string) => map.get(k) ?? null,
    setItem: (k: string, v: string) => void map.set(k, v),
    removeItem: (k: string) => void map.delete(k),
    clear: () => map.clear(),
    key: (i: number) => [...map.keys()][i] ?? null,
    get length() {
      return map.size;
    },
  };
  (globalThis as { localStorage?: unknown }).localStorage = storage;
  return map;
}

const KEY = "zest.providerVerify";

describe("provider verification memory", () => {
  beforeEach(() => installStorage());

  it("remembers a success and lets the caller skip re-probing", () => {
    markProviderVerified("claude");
    assert.equal(recentVerifySucceeded("claude"), true);
    assert.equal(recentVerifyFailed("claude"), false);
  });

  it("remembers a failure without claiming success", () => {
    markProviderVerifyFailed("claude");
    assert.equal(recentVerifyFailed("claude"), true);
    assert.equal(recentVerifySucceeded("claude"), false);
  });

  it("knows nothing about a provider it has never seen", () => {
    assert.equal(recentVerifySucceeded("codex"), false);
    assert.equal(recentVerifyFailed("codex"), false);
    assert.equal(getProviderVerify("codex"), null);
  });

  it("stops trusting a verdict once the window passes", () => {
    // A stale success must re-probe rather than assume the account still works.
    const stored = installStorage();
    stored.set(
      KEY,
      JSON.stringify({ claude: { at: Date.now() - VERIFY_TTL_MS - 1, ok: true } })
    );
    assert.equal(recentVerifySucceeded("claude"), false);

    stored.set(
      KEY,
      JSON.stringify({ claude: { at: Date.now() - VERIFY_TTL_MS + 5_000, ok: true } })
    );
    assert.equal(recentVerifySucceeded("claude"), true);
  });

  it("lets the later verdict win for one provider", () => {
    markProviderVerified("claude");
    markProviderVerifyFailed("claude");
    assert.equal(recentVerifyFailed("claude"), true);
    assert.equal(recentVerifySucceeded("claude"), false);

    markProviderVerified("claude");
    assert.equal(recentVerifySucceeded("claude"), true);
    assert.equal(recentVerifyFailed("claude"), false);
  });

  it("keeps providers independent", () => {
    markProviderVerified("codex");
    markProviderVerifyFailed("claude");
    assert.equal(recentVerifySucceeded("codex"), true);
    assert.equal(recentVerifyFailed("claude"), true);
  });

  it("forgets on request", () => {
    markProviderVerified("claude");
    forgetProviderVerify("claude");
    assert.equal(getProviderVerify("claude"), null);
    assert.equal(recentVerifySucceeded("claude"), false);
  });

  it("treats corrupt storage as unknown rather than throwing", () => {
    // Launch must survive a hand-edited or half-written entry.
    for (const raw of [
      "not json",
      "null",
      "[]",
      '{"claude":"nope"}',
      '{"claude":{"at":"soon","ok":true}}',
      '{"claude":{"at":123}}',
      '{"claude":{"ok":true}}',
    ]) {
      installStorage({ [KEY]: raw });
      assert.equal(recentVerifySucceeded("claude"), false, raw);
      assert.equal(recentVerifyFailed("claude"), false, raw);
      assert.equal(getProviderVerify("claude"), null, raw);
    }
  });

  it("survives storage that throws on every access", () => {
    // A webview can deny storage outright; verification must degrade, not crash.
    (globalThis as { localStorage?: unknown }).localStorage = {
      getItem() {
        throw new Error("denied");
      },
      setItem() {
        throw new Error("denied");
      },
      removeItem() {
        throw new Error("denied");
      },
    };
    assert.doesNotThrow(() => markProviderVerified("claude"));
    assert.equal(recentVerifySucceeded("claude"), false);
    assert.equal(recentVerifyFailed("claude"), false);
  });
});
