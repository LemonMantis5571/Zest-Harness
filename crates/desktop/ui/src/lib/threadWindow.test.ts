import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  appendNewerMessages,
  aroundUserTurns,
  firstLoadedMessageId,
  lastLoadedMessageId,
  newerUserTurns,
  olderUserTurns,
  prependOlderMessages,
  tailUserTurns,
} from "./threadWindow.ts";
import type { ChatMessage } from "./types.ts";

function user(id: string, text: string): ChatMessage {
  return { id, role: "user", text };
}

function assistant(id: string, text: string): ChatMessage {
  return {
    id,
    role: "assistant",
    text,
    thinking: "",
    tools: [],
    streaming: false,
  };
}

function threadWithUsers(count: number): ChatMessage[] {
  const messages: ChatMessage[] = [];
  for (let index = 1; index <= count; index += 1) {
    messages.push(user(`u${index}`, `Turn ${index} prompt`));
    messages.push(assistant(`a${index}`, `Turn ${index} reply`));
  }
  return messages;
}

describe("thread window merge", () => {
  it("prepends older turns without duplicating an overlapping cursor", () => {
    const current = [user("u6", "six"), user("u7", "seven")];
    const older = [user("u4", "four"), user("u5", "five"), user("u6", "six-again")];
    const merged = prependOlderMessages(current, older);
    assert.deepEqual(
      merged.map((message) => message.id),
      ["u4", "u5", "u6", "u7"]
    );
    assert.equal(merged[2]?.role === "user" ? merged[2].text : null, "six");
  });

  it("opens a long thread on the last ten user turns", () => {
    const first = tailUserTurns(threadWithUsers(15), 10);
    assert.equal(first.messages[0]?.id, "u6");
    assert.equal(first.messages.at(-1)?.id, "a15");
    assert.equal(first.hasOlder, true);
    assert.equal(first.hasNewer, false);
    assert.equal(first.hiddenUserTurns, 5);

    const older = olderUserTurns(threadWithUsers(15), "u6", 20);
    assert.equal(older.messages[0]?.id, "u1");
    assert.equal(older.messages.at(-1)?.id, "a5");
    assert.equal(older.hasOlder, false);
    assert.equal(older.hasNewer, false);
  });

  it("opens a search hit near the start on that turn", () => {
    const first = aroundUserTurns(threadWithUsers(15), "u1", 10);
    assert.equal(first.messages[0]?.id, "u1");
    assert.equal(first.messages.at(-1)?.id, "a10");
    assert.equal(first.hasOlder, false);
    assert.equal(first.hasNewer, true);

    const newer = newerUserTurns(threadWithUsers(15), "a10", 20);
    assert.equal(newer.messages[0]?.id, "u11");
    assert.equal(newer.messages.at(-1)?.id, "a15");
    assert.equal(newer.hasNewer, false);
    assert.deepEqual(
      appendNewerMessages(first.messages, newer.messages).map((message) => message.id),
      threadWithUsers(15).map((message) => message.id)
    );
  });

  it("leaves the current window alone when the page is empty", () => {
    const current = [user("u1", "one")];
    assert.equal(prependOlderMessages(current, []), current);
    assert.equal(firstLoadedMessageId(current), "u1");
    assert.equal(lastLoadedMessageId(current), "u1");
    assert.equal(firstLoadedMessageId([]), null);
    assert.equal(lastLoadedMessageId([]), null);
    assert.equal(appendNewerMessages(current, []), current);
  });
});
