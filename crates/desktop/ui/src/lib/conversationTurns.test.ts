import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { buildConversationTurns } from "./conversationTurns.ts";
import type { ChatMessage, ThreadCheckpoint } from "./types.ts";

function user(id: string, text: string): ChatMessage {
  return { id, role: "user", text };
}

function assistant(
  id: string,
  text: string,
  tools: Extract<ChatMessage, { role: "assistant" }>["tools"] = [],
  extra: Partial<Extract<ChatMessage, { role: "assistant" }>> = {}
): ChatMessage {
  return {
    id,
    role: "assistant",
    text,
    thinking: "",
    tools,
    streaming: false,
    ...extra,
  };
}

describe("conversation turn index", () => {
  it("groups assistant work under the preceding user turn and counts tools", () => {
    const turns = buildConversationTurns([
      user("u1", "  Inspect the repository\nplease. "),
      assistant("a1", "I inspected it.", [
        { id: "t1", name: "list_dir", status: "done" },
        { id: "t2", name: "read_file", status: "done" },
      ]),
      user("u2", "Make the change"),
      assistant("a2", "Working…", [], { streaming: true }),
    ]);

    assert.deepEqual(
      turns.map(({ messageId, number, preview, toolCount, status }) => ({
        messageId,
        number,
        preview,
        toolCount,
        status,
      })),
      [
        {
          messageId: "u1",
          number: 1,
          preview: "Inspect the repository please.",
          toolCount: 2,
          status: "done",
        },
        {
          messageId: "u2",
          number: 2,
          preview: "Make the change",
          toolCount: 0,
          status: "working",
        },
      ]
    );
  });

  it("marks a turn's checkpoint without changing its navigation identity", () => {
    const checkpoints = [
      {
        id: "cp-1",
        createdAt: 1_700_000_000,
        label: "Turn 1",
        messageCount: 2,
        agentMessageCount: 1,
        anchorMessageId: "u1",
        preview: "Inspect the repository",
        kind: "turn",
      },
    ] as ThreadCheckpoint[];

    const [turn] = buildConversationTurns(
      [user("u1", "Inspect the repository"), assistant("a1", "Done")],
      checkpoints
    );

    assert.equal(turn.messageId, "u1");
    assert.equal(turn.checkpoint?.id, "cp-1");
    assert.equal(turn.checkpoint?.createdAt, 1_700_000_000);
  });

  it("uses a safe fallback for empty prompts and preserves error status", () => {
    const [turn] = buildConversationTurns([
      user("u1", "   "),
      assistant("a1", "The turn failed.", [], {
        error: "provider unavailable",
      }),
    ]);

    assert.equal(turn.preview, "Empty prompt");
    assert.equal(turn.status, "error");
  });
});
