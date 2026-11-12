import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  buildTrace,
  orderedSteps,
  parseAgentTitle,
  subagentDuration,
  type TraceItem,
} from "./assistantTrace.ts";
import { initialChatUiState, reduceChatEvent } from "./chatReducer.ts";
import { COLLAPSE_THRESHOLD } from "./toolRuns.ts";
import type {
  AssistantStep,
  ChatEvent,
  ChatMessage,
  ProviderActivityPart,
  ToolPart,
} from "./types.ts";

type Assistant = Extract<ChatMessage, { role: "assistant" }>;

function tool(over: Partial<ToolPart> & { id: string }): ToolPart {
  return { name: "read_file", status: "done", ...over };
}

function many(n: number, prefix = "t", over: Partial<ToolPart> = {}): ToolPart[] {
  return Array.from({ length: n }, (_, i) => tool({ id: `${prefix}${i}`, ...over }));
}

function activity(
  over: Partial<ProviderActivityPart> & { id: string }
): ProviderActivityPart {
  return { title: "Read src/lib.rs", status: "done", ...over };
}

function message(over: Partial<Assistant> = {}): Assistant {
  return {
    id: "a1",
    role: "assistant",
    text: "",
    thinking: "",
    tools: [],
    streaming: false,
    ...over,
  };
}

/** The flat sequence of ids a trace shows, groups opened. */
function ids(items: TraceItem[]): string[] {
  return items.flatMap((item) => {
    switch (item.kind) {
      case "thinking":
        return [item.key];
      case "tool":
      case "subagent-tool":
        return [item.tool.id];
      case "tool-group":
        return item.tools.map((t) => t.id);
      case "activity":
        return [item.activity.id];
      case "activity-group":
        return item.activities.map((a) => a.id);
      case "subagent-activity":
        return [item.node.activity.id];
    }
  });
}

describe("orderedSteps", () => {
  it("keeps later reasoning live after commentary and settles it when new text arrives", () => {
    const identity = { session_id: "s", thread_id: "t", turn_id: "turn", message_id: "a" };
    let state = initialChatUiState();
    const apply = (event: ChatEvent) => {
      state = reduceChatEvent(state, event).state;
      const assistant = state.messages[0];
      assert.ok(assistant?.role === "assistant");
      return orderedSteps(assistant).filter((step) => step.kind === "thinking");
    };
    apply({ kind: "thinking_delta", ...identity, text: "Look first." });
    apply({ kind: "text_delta", ...identity, text: "I will inspect the file." });
    apply({ kind: "tool_call_start", ...identity, id: "read", name: "read_file" });
    apply({ kind: "tool_call_result", ...identity, id: "read", name: "read_file", summary: "ok", isError: false });

    const thinking = apply({ kind: "thinking_delta", ...identity, text: "Now check " });
    assert.deepEqual(thinking.map((step) => [step.text, step.live]), [
      ["Look first.", false],
      ["Now check ", true],
    ]);
    assert.equal(apply({ kind: "thinking_delta", ...identity, text: "the caller." }).at(-1)?.live, true);
    assert.equal(apply({ kind: "text_delta", ...identity, text: "The caller is correct." }).at(-1)?.live, false);
    assert.ok(apply({ kind: "done", ...identity }).every((step) => !step.live));
  });

  it("follows the recorded order, thinking between tools included", () => {
    const steps: AssistantStep[] = [
      { kind: "thinking", text: "Look first." },
      { kind: "tool", id: "t1" },
      { kind: "thinking", text: "Now edit." },
      { kind: "activity", id: "x" },
    ];
    const resolved = orderedSteps(
      message({
        thinking: "Look first.Now edit.",
        tools: [tool({ id: "t1" })],
        providerActivity: [activity({ id: "x" })],
        steps,
      })
    );
    assert.deepEqual(
      resolved.map((step) =>
        step.kind === "thinking" ? step.text : step.kind === "tool" ? step.tool.id : step.activity.id
      ),
      ["Look first.", "t1", "Now edit.", "x"]
    );
  });

  it("keeps the old fixed order for threads saved without steps", () => {
    const resolved = orderedSteps(
      message({
        thinking: "plan",
        tools: [tool({ id: "t1" })],
        providerActivity: [activity({ id: "x" })],
      })
    );
    assert.deepEqual(resolved.map((step) => step.kind), ["thinking", "tool", "activity"]);
  });

  it("never drops a part no step names", () => {
    const resolved = orderedSteps(
      message({ tools: [tool({ id: "t1" }), tool({ id: "t2" })], steps: [{ kind: "tool", id: "t2" }] })
    );
    assert.deepEqual(
      resolved.map((step) => (step.kind === "tool" ? step.tool.id : "")),
      ["t2", "t1"]
    );
  });

  it("marks only the newest block live, and only before anything follows it", () => {
    const steps: AssistantStep[] = [
      { kind: "thinking", text: "a" },
      { kind: "tool", id: "t1" },
      { kind: "thinking", text: "b" },
    ];
    const live = orderedSteps(message({ tools: [tool({ id: "t1" })], steps, streaming: true }));
    assert.deepEqual(
      live.map((step) => step.kind === "thinking" && step.live),
      [false, false, true]
    );
    const answering = orderedSteps(
      message({ tools: [tool({ id: "t1" })], steps, streaming: true, text: "Done." })
    );
    assert.ok(answering.every((step) => !(step.kind === "thinking" && step.live)));
  });
});

describe("subagentDuration", () => {
  it("does not invent elapsed time for a terminal tool or activity without an end", () => {
    for (const status of ["done", "error", "awaiting_approval"] as const) {
      assert.equal(subagentDuration({ status, startedAt: 1_000 }, 86_401_000), undefined);
    }
  });

  it("keeps a known terminal duration fixed and lets a running subagent advance", () => {
    const stopped = { status: "error" as const, startedAt: 1_000, finishedAt: 6_000 };
    assert.equal(subagentDuration(stopped, 6_000), "5s");
    assert.equal(subagentDuration(stopped, 86_401_000), "5s");
    assert.equal(subagentDuration({ status: "running", startedAt: 1_000 }, 7_000), "6s");
    assert.equal(subagentDuration({ status: "error" }, 7_000), undefined);
  });
});

describe("buildTrace", () => {
  it("leaves a single call as a line", () => {
    const items = buildTrace(message({ tools: many(1) }));
    assert.deepEqual(items.map((item) => item.kind), ["tool"]);
  });

  it("folds a run once it reaches the threshold", () => {
    const items = buildTrace(message({ tools: many(COLLAPSE_THRESHOLD) }));
    assert.deepEqual(items.map((item) => item.kind), ["tool-group"]);
  });

  it("keeps HTML cards visible between folded lookup groups", () => {
    const artifact = tool({
      id: "html",
      name: "html_render",
      metadata: { kind: "html_document", title: "Counter", html: "<p>0</p>" },
    });
    const tools = [...many(2, "a"), artifact, ...many(2, "b")];
    const items = buildTrace(message({ tools }));
    assert.deepEqual(items.map((item) => item.kind), ["tool-group", "tool", "tool-group"]);
    assert.deepEqual(ids(items), tools.map((t) => t.id));
  });

  it("never folds an awaiting-approval call", () => {
    const tools = [
      ...many(3),
      tool({ id: "ask", name: "bash", status: "awaiting_approval" }),
      ...many(2, "after"),
    ];
    const items = buildTrace(message({ tools }));
    assert.deepEqual(items.map((item) => item.kind), ["tool-group", "tool", "tool-group"]);
  });

  it("never folds a delegation into an N tool calls group", () => {
    const tools = [
      ...many(3),
      tool({
        id: "worker",
        name: "delegate_external",
        metadata: { kind: "delegation", provider_id: "codex", model: "gpt-5" },
      }),
      tool({ id: "card", name: "delegate_feature" }),
      ...many(3, "after"),
    ];
    const items = buildTrace(message({ tools }));
    assert.deepEqual(items.map((item) => item.kind), [
      "tool-group",
      "subagent-tool",
      "subagent-tool",
      "tool-group",
    ]);
    assert.deepEqual(ids(items), tools.map((t) => t.id));
  });

  it("breaks a run where the model thought again", () => {
    const tools = many(4);
    const steps: AssistantStep[] = [
      { kind: "tool", id: "t0" },
      { kind: "tool", id: "t1" },
      { kind: "thinking", text: "Hm." },
      { kind: "tool", id: "t2" },
      { kind: "tool", id: "t3" },
    ];
    const items = buildTrace(message({ tools, steps }));
    assert.deepEqual(items.map((item) => item.kind), ["tool-group", "thinking", "tool-group"]);
  });

  it("gives a Claude Code Agent row its own subagent line with its steps nested", () => {
    const providerActivity = [
      activity({ id: "r0", title: "Read README.md" }),
      activity({ id: "agent", title: "Agent Map the parser: Read src/lib.rs", status: "running" }),
      activity({ id: "child-1", title: "Read src/lib.rs", parentId: "agent" }),
      activity({ id: "child-2", title: "Grep parse", parentId: "agent" }),
      activity({ id: "r1", title: "Bash cargo test" }),
      activity({ id: "r2", title: "Bash cargo check" }),
    ];
    const items = buildTrace(message({ providerActivity }));
    assert.deepEqual(items.map((item) => item.kind), [
      "activity",
      "subagent-activity",
      "activity-group",
    ]);
    const agent = items[1];
    assert.ok(agent.kind === "subagent-activity");
    assert.deepEqual(
      agent.node.children.map((child) => child.activity.id),
      ["child-1", "child-2"]
    );
  });

  it("keeps a child whose parent never arrived as a top-level line", () => {
    const items = buildTrace(
      message({ providerActivity: [activity({ id: "orphan", parentId: "gone" })] })
    );
    assert.deepEqual(items.map((item) => item.kind), ["activity"]);
  });

  it("handles a turn with nothing to show", () => {
    assert.deepEqual(buildTrace(message()), []);
  });
});

describe("parseAgentTitle", () => {
  it("splits the description from the last reported step", () => {
    assert.deepEqual(parseAgentTitle("Agent Map the parser: Bash cargo test"), {
      description: "Map the parser",
      progress: "Bash cargo test",
    });
    assert.deepEqual(parseAgentTitle("Task Review the diff"), {
      description: "Review the diff",
    });
  });

  it("does not mistake other tools for subagents", () => {
    assert.equal(parseAgentTitle("Agentic search"), null);
    assert.equal(parseAgentTitle("Read Agent.md"), null);
  });
});
