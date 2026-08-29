import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  activeThreadIds,
  currentTurnAction,
  elapsedLabel,
  formatActivityAction,
  reduceThreadActivity,
  type ThreadActivityMap,
} from "./threadActivity.ts";
import type { ChatEvent } from "./types.ts";

const T0 = 1_700_000_000_000;

function ident(threadId: string) {
  return { session_id: "s", thread_id: threadId, turn_id: "t" };
}

function feed(events: ChatEvent[], start: ThreadActivityMap = {}): ThreadActivityMap {
  return events.reduce((map, event, index) => reduceThreadActivity(map, event, T0 + index * 1000), start);
}

describe("thread activity", () => {
  it("follows a chat you are not looking at", () => {
    // The point of the whole module: these events belong to another thread and
    // the transcript reducer throws them away.
    const map = feed([
      { kind: "user", ...ident("other"), message_id: "u1", text: "go" },
      { kind: "assistant_start", ...ident("other"), message_id: "a1" },
      { kind: "tool_call_start", ...ident("other"), message_id: "a1", name: "read_file", id: "c1" },
    ] as ChatEvent[]);

    assert.equal(map.other.state, "working");
    assert.equal(map.other.tool, "read_file");
    assert.equal(map.other.lastAction, "read_file");
    assert.equal(map.other.startedAt, T0, "the clock starts at the user's message");
  });

  it("keeps threads apart", () => {
    const map = feed([
      { kind: "user", ...ident("a"), message_id: "u1", text: "x" },
      { kind: "user", ...ident("b"), message_id: "u2", text: "y" },
      { kind: "done", ...ident("a"), message_id: "a1" },
    ] as ChatEvent[]);

    assert.equal(map.a.state, "idle");
    assert.equal(map.b.state, "working");
    assert.deepEqual(activeThreadIds(map), ["b"]);
  });

  it("clears the running tool when it finishes but stays on the turn", () => {
    const map = feed([
      { kind: "user", ...ident("a"), message_id: "u1", text: "x" },
      { kind: "tool_call_start", ...ident("a"), message_id: "a1", name: "bash", id: "c1" },
      {
        kind: "tool_call_result",
        ...ident("a"),
        message_id: "a1",
        name: "bash",
        id: "c1",
        summary: "ok",
        isError: false,
      },
    ] as ChatEvent[]);

    assert.equal(map.a.tool, undefined, "no tool is running now");
    assert.equal(map.a.state, "working", "but the turn has not ended");
    assert.equal(map.a.lastAction, "bash");
  });

  it("says when a chat is stuck waiting on a person", () => {
    // The state most worth surfacing: nothing will happen until you answer.
    const map = feed([
      { kind: "user", ...ident("a"), message_id: "u1", text: "x" },
      {
        kind: "approval_needed",
        ...ident("a"),
        message_id: "a1",
        approval_id: "ap1",
        tool_name: "write_file",
        tool_call_id: "c1",
        risk: "write",
        path: "a.txt",
        summary: "",
        diff: "",
      },
    ] as ChatEvent[]);

    assert.equal(map.a.state, "awaiting_approval");
    assert.equal(map.a.lastAction, "write_file needs approval");
  });

  it("marks a failed tool without ending the turn", () => {
    const map = feed([
      { kind: "user", ...ident("a"), message_id: "u1", text: "x" },
      {
        kind: "tool_call_result",
        ...ident("a"),
        message_id: "a1",
        name: "bash",
        id: "c1",
        summary: "boom",
        isError: true,
      },
    ] as ChatEvent[]);
    assert.equal(map.a.lastAction, "bash failed");
    assert.equal(map.a.state, "working");
  });

  it("goes idle on every way a turn can end", () => {
    for (const kind of ["done", "error", "cancelled"] as const) {
      const map = feed([
        { kind: "user", ...ident("a"), message_id: "u1", text: "x" },
        { kind, ...ident("a"), message_id: "a1", message: "m" },
      ] as ChatEvent[]);
      assert.equal(map.a.state, "idle", kind);
    }
  });

  it("returns the same object when nothing changed", () => {
    // This runs on every delta of every live chat, so a new object each time
    // would re-render the sidebar continuously.
    const first = feed([{ kind: "user", ...ident("a"), message_id: "u1", text: "x" }] as ChatEvent[]);
    const again = reduceThreadActivity(
      first,
      { kind: "assistant_start", ...ident("a"), message_id: "a1" } as ChatEvent,
      T0 + 5000
    );
    assert.equal(again, first, "identity preserved");
  });

  it("ignores an event with no thread", () => {
    const map = reduceThreadActivity({}, { kind: "done" } as unknown as ChatEvent, T0);
    assert.deepEqual(map, {});
  });
});

describe("currentTurnAction", () => {
  it("prefers a running tool over a finished one", () => {
    assert.equal(
      currentTurnAction({
        role: "assistant",
        tools: [
          { name: "read_file", status: "done" },
          { name: "web_search", status: "running" },
        ],
      }),
      "web search"
    );
  });

  it("uses a running provider step when no tool is live", () => {
    assert.equal(
      currentTurnAction({
        role: "assistant",
        tools: [{ name: "read_file", status: "done" }],
        providerActivity: [{ title: "web search", status: "running" }],
      }),
      "web search"
    );
  });

  it("falls back to the thread's running tool before assistant content exists", () => {
    assert.equal(
      currentTurnAction(undefined, { state: "working", tool: "git_status" }),
      "git status"
    );
  });

  it("stays quiet once nothing is running", () => {
    assert.equal(
      currentTurnAction(
        {
          role: "assistant",
          tools: [{ name: "web_search", status: "done" }],
        },
        { state: "working", lastAction: "web_search" }
      ),
      undefined
    );
  });
});

describe("formatActivityAction", () => {
  it("turns tool ids into readable labels", () => {
    assert.equal(formatActivityAction("web_search"), "web search");
    assert.equal(formatActivityAction("mcp__Haiku__manifest"), "Haiku · manifest");
  });
});

describe("elapsedLabel", () => {
  it("reads naturally at each scale", () => {
    assert.equal(elapsedLabel(T0, T0 + 8_000), "8s");
    assert.equal(elapsedLabel(T0, T0 + 411_900), "6m 51s");
    assert.equal(elapsedLabel(T0, T0 + 3_840_000), "1h 04m");
  });

  it("has nothing to say without a start, or before it", () => {
    assert.equal(elapsedLabel(undefined, T0), undefined);
    // A clock that disagrees with the backend must not render "-3s".
    assert.equal(elapsedLabel(T0, T0 - 3000), undefined);
  });
});
