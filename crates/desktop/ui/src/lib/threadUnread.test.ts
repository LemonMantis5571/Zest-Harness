import assert from "node:assert/strict";
import { afterEach, beforeEach, describe, it } from "node:test";

import {
  MAX_UNREAD_THREADS,
  UNREAD_THREADS_STORAGE_KEY,
  getUnreadThreads,
  markThreadSeen,
  recordUnreadEvent,
  reduceUnreadThreads,
  withThreadSeen,
  type UnreadThreads,
} from "./threadUnread.ts";
import type { ChatEvent } from "./types.ts";

function ended(threadId: string, kind: "done" | "error" | "cancelled" = "done"): ChatEvent {
  return {
    kind,
    session_id: "s",
    thread_id: threadId,
    turn_id: "t",
    message_id: "m",
    message: "boom",
  } as ChatEvent;
}

function installStorage(initial: Record<string, string> = {}) {
  const map = new Map(Object.entries(initial));
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (key: string) => map.get(key) ?? null,
    setItem: (key: string, value: string) => void map.set(key, value),
    removeItem: (key: string) => void map.delete(key),
    clear: () => map.clear(),
    key: (index: number) => [...map.keys()][index] ?? null,
    get length() {
      return map.size;
    },
  };
  return map;
}

const NONE: UnreadThreads = new Set();

describe("reduceUnreadThreads", () => {
  it("marks a chat whose turn ended while another chat was open", () => {
    const next = reduceUnreadThreads(NONE, ended("a"), "b");
    assert.deepEqual([...next], ["a"]);
  });

  it("leaves the chat you are looking at alone", () => {
    assert.equal(reduceUnreadThreads(NONE, ended("a"), "a"), NONE);
  });

  it("counts a turn that failed, since there is something to read", () => {
    assert.deepEqual([...reduceUnreadThreads(NONE, ended("a", "error"), "b")], ["a"]);
  });

  it("ignores a cancel, which you did yourself", () => {
    assert.equal(reduceUnreadThreads(NONE, ended("a", "cancelled"), "b"), NONE);
  });

  it("ignores events that do not end a turn", () => {
    const start = {
      kind: "assistant_start",
      session_id: "s",
      thread_id: "a",
      turn_id: "t",
      message_id: "m",
    } as ChatEvent;
    assert.equal(reduceUnreadThreads(NONE, start, "b"), NONE);
  });

  it("keeps the same set when the chat is already marked", () => {
    const once = reduceUnreadThreads(NONE, ended("a"), "b");
    assert.equal(reduceUnreadThreads(once, ended("a"), "b"), once);
  });

  it("drops the oldest marks past the cap", () => {
    let unread: UnreadThreads = NONE;
    for (let index = 0; index <= MAX_UNREAD_THREADS; index += 1) {
      unread = reduceUnreadThreads(unread, ended(`t${index}`), null);
    }
    assert.equal(unread.size, MAX_UNREAD_THREADS);
    assert.ok(!unread.has("t0"));
    assert.ok(unread.has(`t${MAX_UNREAD_THREADS}`));
  });
});

describe("withThreadSeen", () => {
  it("clears a chat once you open it", () => {
    const unread = new Set(["a", "b"]);
    assert.deepEqual([...withThreadSeen(unread, "a")], ["b"]);
  });

  it("keeps the same set when there was nothing to clear", () => {
    const unread = new Set(["a"]);
    assert.equal(withThreadSeen(unread, "z"), unread);
  });
});

describe("unread chat store", () => {
  let previousStorage: unknown;

  beforeEach(() => {
    previousStorage = (globalThis as { localStorage?: unknown }).localStorage;
  });

  afterEach(() => {
    const target = globalThis as { localStorage?: unknown };
    if (previousStorage === undefined) delete target.localStorage;
    else target.localStorage = previousStorage;
  });

  it("survives a restart by persisting to storage", () => {
    const storage = installStorage();
    recordUnreadEvent(ended("a"), "b");
    assert.equal(storage.get(UNREAD_THREADS_STORAGE_KEY), JSON.stringify(["a"]));
    assert.ok(getUnreadThreads().has("a"));

    markThreadSeen("a");
    assert.equal(storage.get(UNREAD_THREADS_STORAGE_KEY), JSON.stringify([]));
    assert.equal(getUnreadThreads().size, 0);
  });

  it("returns the same snapshot until something changes", () => {
    installStorage({ [UNREAD_THREADS_STORAGE_KEY]: JSON.stringify(["a"]) });
    const first = getUnreadThreads();
    assert.equal(getUnreadThreads(), first);
    recordUnreadEvent(ended("b"), null);
    assert.notEqual(getUnreadThreads(), first);
  });

  it("reads damaged storage as nothing unread", () => {
    installStorage({ [UNREAD_THREADS_STORAGE_KEY]: "{not json" });
    assert.equal(getUnreadThreads().size, 0);
    installStorage({ [UNREAD_THREADS_STORAGE_KEY]: JSON.stringify(["a", 4, "", null]) });
    assert.deepEqual([...getUnreadThreads()], ["a"]);
  });
});
