import assert from "node:assert/strict";
import { afterEach, beforeEach, describe, it } from "node:test";

import {
  CHAT_GROUPING_STORAGE_KEY,
  CHAT_VIEW_STORAGE_KEY,
  DEFAULT_CHAT_GROUPING,
  DEFAULT_CHAT_VIEW_MODE,
  applyChatViewMode,
  getSavedChatGrouping,
  getSavedChatViewMode,
  saveChatGrouping,
} from "./chatView.ts";

function installStorage(initial: Record<string, string> = {}) {
  const map = new Map(Object.entries(initial));
  const storage = {
    getItem: (key: string) => map.get(key) ?? null,
    setItem: (key: string, value: string) => void map.set(key, value),
    removeItem: (key: string) => void map.delete(key),
    clear: () => map.clear(),
    key: (index: number) => [...map.keys()][index] ?? null,
    get length() {
      return map.size;
    },
  };
  (globalThis as { localStorage?: unknown }).localStorage = storage;
  return map;
}

describe("chat history view preference", () => {
  let previousStorage: unknown;

  beforeEach(() => {
    previousStorage = (globalThis as { localStorage?: unknown }).localStorage;
    installStorage();
  });

  afterEach(() => {
    const target = globalThis as { localStorage?: unknown };
    if (previousStorage === undefined) delete target.localStorage;
    else target.localStorage = previousStorage;
  });

  it("uses the project tree when no preference is saved", () => {
    assert.equal(getSavedChatViewMode(), DEFAULT_CHAT_VIEW_MODE);
  });

  it("persists the selected compact list", () => {
    const storage = installStorage();
    assert.equal(applyChatViewMode("compact"), "compact");
    assert.equal(storage.get(CHAT_VIEW_STORAGE_KEY), "compact");
    assert.equal(getSavedChatViewMode(), "compact");
  });

  it("ignores an unknown stored mode", () => {
    installStorage({ [CHAT_VIEW_STORAGE_KEY]: "timeline" });
    assert.equal(getSavedChatViewMode(), DEFAULT_CHAT_VIEW_MODE);
  });

  it("groups the compact list by project until told otherwise", () => {
    assert.equal(getSavedChatGrouping(), DEFAULT_CHAT_GROUPING);
    assert.equal(DEFAULT_CHAT_GROUPING, "project");
  });

  it("remembers grouping by status", () => {
    const storage = installStorage();
    assert.equal(saveChatGrouping("status"), "status");
    assert.equal(storage.get(CHAT_GROUPING_STORAGE_KEY), "status");
    assert.equal(getSavedChatGrouping(), "status");
  });

  it("ignores an unknown stored grouping", () => {
    installStorage({ [CHAT_GROUPING_STORAGE_KEY]: "provider" });
    assert.equal(getSavedChatGrouping(), DEFAULT_CHAT_GROUPING);
  });
});
