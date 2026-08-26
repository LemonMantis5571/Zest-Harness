import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  enqueueThreadTurn,
  hasResumableThreadTurn,
  peekThreadTurn,
  removeThreadTurn,
  threadQueueCount,
  updateThreadTurn,
  type QueuedTurn,
} from "./threadQueue.ts";

function turn(id: string, threadId = "thread-a"): QueuedTurn {
  return {
    id,
    threadId,
    text: id,
    attachments: [],
    createdAt: Number(id.slice(-1)) || 0,
  };
}

describe("thread queue", () => {
  it("keeps queued turns FIFO and isolated by thread", () => {
    let queues = enqueueThreadTurn({}, "thread-a", turn("turn-1"));
    queues = enqueueThreadTurn(queues, "thread-b", turn("turn-2", "thread-b"));
    queues = enqueueThreadTurn(queues, "thread-a", turn("turn-3"));

    assert.equal(peekThreadTurn(queues, "thread-a")?.id, "turn-1");
    assert.equal(peekThreadTurn(queues, "thread-b")?.id, "turn-2");
    assert.equal(threadQueueCount(queues, "thread-a"), 2);
    assert.equal(threadQueueCount(queues, "missing"), 0);
  });

  it("removes only the delivered turn and drops empty queues", () => {
    let queues = enqueueThreadTurn({}, "thread-a", turn("turn-1"));
    queues = enqueueThreadTurn(queues, "thread-a", turn("turn-2"));

    const next = removeThreadTurn(queues, "thread-a", "turn-1");
    assert.equal(peekThreadTurn(next, "thread-a")?.id, "turn-2");
    assert.equal(threadQueueCount(next, "thread-a"), 1);

    const empty = removeThreadTurn(next, "thread-a", "turn-2");
    assert.equal(threadQueueCount(empty, "thread-a"), 0);
    assert.equal(Object.hasOwn(empty, "thread-a"), false);
  });

  it("does not mutate the input map when removing an unknown turn", () => {
    const queues = enqueueThreadTurn({}, "thread-a", turn("turn-1"));
    assert.equal(removeThreadTurn(queues, "thread-a", "missing"), queues);
  });

  it("edits a queued turn without changing its position", () => {
    let queues = enqueueThreadTurn({}, "thread-a", turn("turn-1"));
    queues = enqueueThreadTurn(queues, "thread-a", turn("turn-2"));

    const next = updateThreadTurn(queues, "thread-a", "turn-2", "updated");
    assert.deepEqual(next["thread-a"], [
      turn("turn-1"),
      { ...turn("turn-2"), text: "updated" },
    ]);
    assert.equal(updateThreadTurn(queues, "thread-a", "missing", "no-op"), queues);
  });

  it("only presents followups as an idle resume action", () => {
    assert.equal(hasResumableThreadTurn([turn("turn-1")]), true);
    assert.equal(
      hasResumableThreadTurn([{ ...turn("turn-2"), target: "steer" }]),
      false
    );
  });
});
