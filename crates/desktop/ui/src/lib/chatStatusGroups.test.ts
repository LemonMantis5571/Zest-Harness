import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  chatProjectLabel,
  chatStatusSection,
  groupChatsByStatus,
  type ChatStatusRow,
} from "./chatStatusGroups.ts";
import type { ThreadActivityMap } from "./threadActivity.ts";
import type { ProjectChats, ThreadSummary } from "./types.ts";

function thread(id: string, updatedAt: number, pinned = false): ThreadSummary {
  return { id, createdAt: 0, updatedAt, pinned, messageCount: 1, title: id };
}

function project(name: string, path: string | null, threads: ThreadSummary[]): ProjectChats {
  return { name, path, active: false, threads };
}

const ids = (rows: ChatStatusRow[]) => rows.map((row) => row.thread.id);

describe("chatStatusSection", () => {
  it("puts approvals and questions under needs input", () => {
    assert.equal(chatStatusSection({ state: "awaiting_approval" }), "needs_input");
    assert.equal(chatStatusSection({ state: "awaiting_input" }), "needs_input");
  });

  it("puts a running turn under working", () => {
    assert.equal(chatStatusSection({ state: "working" }), "working");
  });

  it("puts idle and never-seen chats under recent", () => {
    assert.equal(chatStatusSection({ state: "idle" }), "recent");
    assert.equal(chatStatusSection(undefined), "recent");
  });
});

describe("groupChatsByStatus", () => {
  const app = project("app", "/code/app", [thread("a1", 300), thread("a2", 200)]);
  const lib = project("lib", "/code/lib", [thread("l1", 250, true), thread("l2", 100)]);
  const free = project("Free chats", null, [thread("f1", 400)]);

  it("collects chats from every project into one list per state", () => {
    const activity: ThreadActivityMap = {
      a2: { state: "awaiting_approval", startedAt: 1 },
      l2: { state: "working", startedAt: 2 },
      f1: { state: "idle" },
    };
    const groups = groupChatsByStatus([app, lib, free], activity);
    assert.deepEqual(ids(groups.needs_input), ["a2"]);
    assert.deepEqual(ids(groups.working), ["l2"]);
    assert.deepEqual(ids(groups.recent), ["l1", "f1", "a1"]);
  });

  it("keeps each row's project so the row can name it", () => {
    const groups = groupChatsByStatus([app, free], { f1: { state: "working", startedAt: 1 } });
    assert.equal(groups.working[0].project, free);
    assert.equal(chatProjectLabel(groups.working[0].project), "No project");
    assert.equal(chatProjectLabel(app), "app");
  });

  it("moves a row when its activity changes", () => {
    const working = groupChatsByStatus([app], { a1: { state: "working", startedAt: 1 } });
    assert.deepEqual(ids(working.working), ["a1"]);
    const asking = groupChatsByStatus([app], { a1: { state: "awaiting_input", startedAt: 1 } });
    assert.deepEqual(ids(asking.working), []);
    assert.deepEqual(ids(asking.needs_input), ["a1"]);
    const done = groupChatsByStatus([app], { a1: { state: "idle" } });
    assert.deepEqual(ids(done.recent), ["a1", "a2"]);
  });

  it("orders live sections by when the turn started, newest first", () => {
    const groups = groupChatsByStatus([app, lib], {
      a1: { state: "working", startedAt: 5_000 },
      a2: { state: "working", startedAt: 9_000 },
      l2: { state: "working", startedAt: 7_000 },
    });
    assert.deepEqual(ids(groups.working), ["a2", "l2", "a1"]);
  });

  it("orders recent chats pinned first, then by last update", () => {
    const groups = groupChatsByStatus([app, lib, free], {});
    assert.deepEqual(ids(groups.recent), ["l1", "f1", "a1", "a2", "l2"]);
  });

  it("lists a chat once even if two projects report it", () => {
    const twice = project("again", "/code/again", [thread("a1", 300)]);
    const groups = groupChatsByStatus([app, twice], {});
    assert.deepEqual(ids(groups.recent), ["a1", "a2"]);
  });
});
