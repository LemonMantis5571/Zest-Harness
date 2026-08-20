import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  findApprovalTool,
  initialChatUiState,
  isStaleChatEvent,
  joinThinkingStream,
  markApprovalRunning,
  reduceChatEvent,
  reduceChatEvents,
  restoreApprovalCard,
  retireApprovalCard,
  type ChatUiState,
} from "./chatReducer.ts";
import type { ChatEvent, ChatMessage } from "./types.ts";

function seqId() {
  let n = 0;
  return (prefix: string) => `${prefix}-${++n}`;
}

const ID = {
  session_id: "session-1",
  thread_id: "thread-1",
  turn_id: "turn-1",
};

function reduceAll(events: ChatEvent[], start?: ChatUiState): ChatUiState {
  const newId = seqId();
  let state =
    start ??
    initialChatUiState([], {
      sessionId: ID.session_id,
      threadId: ID.thread_id,
    });
  for (const event of events) {
    state = reduceChatEvent(state, event, { newId }).state;
  }
  return state;
}

function pendingApproval(
  approvalId: string,
  toolCallId: string,
  messageId = "a1"
): ChatEvent {
  return {
    kind: "approval_needed",
    ...ID,
    message_id: messageId,
    approval_id: approvalId,
    tool_name: "bash",
    tool_call_id: toolCallId,
    risk: "exec",
    path: "",
    summary: "node --version",
    diff: "",
  };
}

function assistant(state: ChatUiState, index = 0): Extract<ChatMessage, { role: "assistant" }> {
  const msg = state.messages.filter((m) => m.role === "assistant")[index];
  assert.ok(msg && msg.role === "assistant");
  return msg;
}

describe("reduceChatEvent characterization", () => {
  it("reduces a coalesced delta frame in order", () => {
    const start = reduceAll([
      { kind: "user", ...ID, message_id: "u1", text: "hello" },
      { kind: "assistant_start", ...ID, message_id: "a1" },
    ]);
    const { state } = reduceChatEvents(start, [
      { kind: "thinking_delta", ...ID, message_id: "a1", text: "Check " },
      { kind: "thinking_delta", ...ID, message_id: "a1", text: "this" },
      { kind: "text_delta", ...ID, message_id: "a1", text: "Hello" },
      { kind: "text_delta", ...ID, message_id: "a1", text: " world" },
    ]);

    assert.equal(assistant(state).thinking, "Check this");
    assert.equal(assistant(state).text, "Hello world");
  });

  it("patches the streaming tail while preserving settled row references", () => {
    const settledUser: ChatMessage = { id: "u1", role: "user", text: "hello" };
    const settledAssistant: ChatMessage = {
      id: "a0",
      role: "assistant",
      text: "settled",
      thinking: "",
      tools: [],
      streaming: false,
    };
    const streamingAssistant: ChatMessage = {
      id: "a1",
      role: "assistant",
      text: "before",
      thinking: "",
      tools: [],
      streaming: true,
    };
    const start = {
      ...initialChatUiState(
        [settledUser, settledAssistant, streamingAssistant],
        { sessionId: ID.session_id, threadId: ID.thread_id }
      ),
      activeAssistantId: "a1",
      currentTurnId: ID.turn_id,
      sending: true,
    };

    const state = reduceChatEvent(start, {
      kind: "text_delta",
      ...ID,
      message_id: "a1",
      text: " after",
    }).state;

    assert.equal(state.messages[0], settledUser);
    assert.equal(state.messages[1], settledAssistant);
    assert.notEqual(state.messages[2], streamingAssistant);
    assert.equal(assistant(state, 1).text, "before after");
  });

  it("still patches an earlier assistant row by id", () => {
    const start = reduceAll([
      { kind: "assistant_start", ...ID, message_id: "a1" },
      { kind: "assistant_start", ...ID, message_id: "a2" },
    ]);
    const state = reduceChatEvent(start, {
      kind: "text_delta",
      ...ID,
      message_id: "a1",
      text: "late update",
    }).state;

    assert.equal(assistant(state, 0).text, "late update");
    assert.equal(assistant(state, 1).text, "");
  });

  it("assistant_start shows empty streaming row before first delta", () => {
    const state = reduceAll([
      { kind: "user", ...ID, message_id: "u1", text: "hello" },
      { kind: "assistant_start", ...ID, message_id: "a1" },
    ]);
    assert.equal(state.messages.length, 2);
    const a = assistant(state);
    assert.equal(a.id, "a1");
    assert.equal(a.text, "");
    assert.equal(a.thinking, "");
    assert.equal(a.streaming, true);
    assert.equal(state.sending, true);
    assert.equal(state.activeAssistantId, "a1");
  });

  it("appends user then streams text and thinking", () => {
    const state = reduceAll([
      { kind: "user", ...ID, message_id: "u1", text: "hello" },
      { kind: "assistant_start", ...ID, message_id: "a1" },
      { kind: "thinking_delta", ...ID, message_id: "a1", text: "hmm " },
      { kind: "thinking_delta", ...ID, message_id: "a1", text: "ok" },
      { kind: "text_delta", ...ID, message_id: "a1", text: "Hi" },
      { kind: "text_delta", ...ID, message_id: "a1", text: " there" },
      { kind: "done", ...ID, message_id: "a1" },
    ]);

    assert.equal(state.messages.length, 2);
    assert.deepEqual(state.messages[0], {
      id: "u1",
      role: "user",
      text: "hello",
    });
    const a = assistant(state);
    assert.equal(a.thinking, "hmm ok");
    assert.equal(a.text, "Hi there");
    assert.equal(a.streaming, false);
    assert.equal(state.activeAssistantId, null);
    assert.equal(state.sending, false);
    assert.equal(state.currentTurnId, null);
  });

  it("shows provider-owned activity without creating a local tool card", () => {
    const state = reduceAll([
      { kind: "user", ...ID, message_id: "u1", text: "inspect" },
      { kind: "assistant_start", ...ID, message_id: "a1" },
      {
        kind: "provider_activity",
        ...ID,
        message_id: "a1",
        id: "claude-tool-1",
        title: "Read",
        status: "running",
      },
      {
        kind: "provider_activity",
        ...ID,
        message_id: "a1",
        id: "claude-tool-1",
        title: "External tool",
        status: "done",
      },
      { kind: "text_delta", ...ID, message_id: "a1", text: "done" },
      { kind: "done", ...ID, message_id: "a1" },
    ]);

    const a = assistant(state);
    assert.deepEqual(a.tools, []);
    assert.deepEqual(a.providerActivity, [
      { id: "claude-tool-1", title: "Read", status: "done" },
    ]);
  });

  it("separates thinking sentence chunks that arrive without whitespace", () => {
    const state = reduceAll([
      { kind: "assistant_start", ...ID, message_id: "a1" },
      {
        kind: "thinking_delta",
        ...ID,
        message_id: "a1",
        text: "Clarifying search scope for web inspection",
      },
      {
        kind: "thinking_delta",
        ...ID,
        message_id: "a1",
        text: "Planning targeted GitHub repository search",
      },
    ]);
    const a = assistant(state);
    assert.equal(
      a.thinking,
      "Clarifying search scope for web inspection\n\nPlanning targeted GitHub repository search"
    );
  });

  it("ignores duplicate user message ids", () => {
    const state = reduceAll([
      { kind: "user", ...ID, message_id: "u1", text: "first" },
      { kind: "user", ...ID, message_id: "u1", text: "second" },
    ]);
    assert.equal(state.messages.length, 1);
    const user = state.messages[0];
    assert.equal(user.role, "user");
    if (user.role === "user") assert.equal(user.text, "first");
    assert.equal(state.activeAssistantId, null);
  });

  it("tracks tool start, approval, and result", () => {
    const state = reduceAll([
      { kind: "user", ...ID, message_id: "u1", text: "edit" },
      {
        kind: "tool_call_start",
        ...ID,
        message_id: "a1",
        name: "write_file",
        id: "t1",
      },
      {
        kind: "approval_needed",
        ...ID,
        message_id: "a1",
        approval_id: "ap1",
        tool_name: "write_file",
        tool_call_id: "t1",
        risk: "write",
        path: "f.txt",
        summary: "write f.txt",
        diff: "+x",
      },
      {
        kind: "tool_call_result",
        ...ID,
        message_id: "a1",
        name: "write_file",
        id: "t1",
        summary: "wrote f.txt",
        isError: false,
      },
      { kind: "done", ...ID, message_id: "a1" },
    ]);

    const tools = assistant(state).tools;
    assert.equal(tools.length, 1);
    assert.equal(tools[0].status, "done");
    assert.equal(tools[0].summary, "wrote f.txt");
    assert.equal(tools[0].approvalId, undefined);
    assert.equal(tools[0].path, "f.txt");
    assert.equal(tools[0].diff, "+x");
  });

  it("renders a structured question and clears it when the tool resumes", () => {
    let state = reduceAll([
      {
        kind: "tool_call_start",
        ...ID,
        message_id: "a1",
        name: "ask_user",
        id: "q-call",
      },
      {
        kind: "question_needed",
        ...ID,
        message_id: "a1",
        question_id: "q-1",
        tool_call_id: "q-call",
        prompt: "Which layout should I use?",
        choices: ["Compact", "Spacious"],
        multiple: false,
      },
    ]);

    const pending = assistant(state);
    assert.equal(pending.streaming, true);
    assert.deepEqual(pending.question, {
      questionId: "q-1",
      toolCallId: "q-call",
      prompt: "Which layout should I use?",
      choices: [
        { value: "Compact", label: "Compact" },
        { value: "Spacious", label: "Spacious" },
      ],
      multiple: false,
      placeholder: undefined,
    });

    state = reduceChatEvent(state, {
      kind: "tool_call_result",
      ...ID,
      message_id: "a1",
      name: "ask_user",
      id: "q-call",
      summary: "Answered",
      isError: false,
    }).state;
    assert.equal(assistant(state).question, undefined);
  });

  it("does not leave a question card after a cancelled turn", () => {
    const state = reduceAll([
      {
        kind: "question_needed",
        ...ID,
        message_id: "a1",
        question_id: "q-1",
        tool_call_id: "q-call",
        prompt: "What should I call it?",
        choices: [],
        multiple: false,
        placeholder: "Project name",
      },
      { kind: "cancelled", ...ID, message_id: "a1" },
    ]);
    assert.equal(assistant(state).question, undefined);
    assert.equal(state.sending, false);
  });

  it("keeps ACP worker provenance metadata on tool cards", () => {
    const state = reduceAll([
      {
        kind: "tool_call_start",
        ...ID,
        message_id: "a1",
        name: "delegate_external",
        id: "t-del",
      },
      {
        kind: "tool_call_update",
        ...ID,
        message_id: "a1",
        id: "t-del",
        metadata: {
          kind: "delegation",
          provider_id: "claude",
          model: "CLI default",
        },
      },
      {
        kind: "tool_call_result",
        ...ID,
        message_id: "a1",
        name: "delegate_external",
        id: "t-del",
        summary: "Delegated to codex · gpt-5.6-sol",
        isError: false,
        metadata: {
          kind: "delegation",
          provider_id: "claude",
          model: "CLI default",
        },
      },
    ]);
    const tool = assistant(state).tools[0];
    assert.equal(tool.metadata?.kind, "delegation");
    if (tool.metadata?.kind === "delegation") {
      assert.equal(tool.metadata.provider_id, "claude");
      assert.equal(tool.metadata.model, "CLI default");
    }
  });

  it("creates a tool card from approval_needed when start was missed", () => {
    const state = reduceAll([
      {
        kind: "approval_needed",
        ...ID,
        message_id: "a1",
        approval_id: "ap1",
        tool_name: "write_file",
        tool_call_id: "t9",
        risk: "write",
        path: "g.txt",
        summary: "write g.txt",
        diff: "",
      },
    ]);
    const tool = assistant(state).tools[0];
    assert.equal(tool.id, "t9");
    assert.equal(tool.status, "awaiting_approval");
    assert.equal(tool.approvalId, "ap1");
  });

  it("marks tool errors and clears approval id", () => {
    const state = reduceAll([
      {
        kind: "tool_call_start",
        ...ID,
        message_id: "a1",
        name: "write_file",
        id: "t1",
      },
      {
        kind: "approval_needed",
        ...ID,
        message_id: "a1",
        approval_id: "ap1",
        tool_name: "write_file",
        tool_call_id: "t1",
        risk: "write",
        path: "f.txt",
        summary: "write",
        diff: "",
      },
      {
        kind: "tool_call_result",
        ...ID,
        message_id: "a1",
        name: "write_file",
        id: "t1",
        summary: "denied",
        isError: true,
      },
    ]);
    assert.equal(assistant(state).tools[0].status, "error");
    assert.equal(assistant(state).tools[0].approvalId, undefined);
  });

  it("done without message_id clears active assistant streaming", () => {
    let state = initialChatUiState([], {
      sessionId: ID.session_id,
      threadId: ID.thread_id,
    });
    state = {
      ...state,
      activeAssistantId: "a1",
      sending: true,
      currentTurnId: ID.turn_id,
      messages: [
        {
          id: "a1",
          role: "assistant",
          text: "partial",
          thinking: "",
          tools: [],
          streaming: true,
        },
      ],
    };
    state = reduceChatEvent(state, {
      kind: "done",
      ...ID,
      message_id: "",
    }).state;
    assert.equal(assistant(state).streaming, false);
    assert.equal(state.activeAssistantId, null);
    assert.equal(state.sending, false);
  });

  it("error attaches to assistant and requests toast effect", () => {
    const { state, effects } = reduceChatEvent(
      initialChatUiState([], {
        sessionId: ID.session_id,
        threadId: ID.thread_id,
      }),
      {
        kind: "error",
        ...ID,
        message_id: "a1",
        message: "boom",
        provider_selection: "deepseek",
      },
      { newId: seqId() }
    );
    assert.equal(assistant(state).error, "boom");
    assert.equal(assistant(state).streaming, false);
    assert.equal(state.sending, false);
    assert.equal(state.activeAssistantId, null);
    assert.equal(effects.errorToast, "boom");
    assert.equal(assistant(state).providerSelection, "deepseek");
  });

  it("does not duplicate tool_call_start for the same id", () => {
    const state = reduceAll([
      {
        kind: "tool_call_start",
        ...ID,
        message_id: "a1",
        name: "grep",
        id: "t1",
      },
      {
        kind: "tool_call_start",
        ...ID,
        message_id: "a1",
        name: "grep",
        id: "t1",
      },
    ]);
    assert.equal(assistant(state).tools.length, 1);
  });

  it("ignores stale session/thread/turn events", () => {
    let state = initialChatUiState([], {
      sessionId: "session-1",
      threadId: "thread-1",
    });
    state = reduceChatEvent(state, {
      kind: "user",
      ...ID,
      message_id: "u1",
      text: "live",
    }).state;
    assert.equal(state.messages.length, 1);

    const staleSession: ChatEvent = {
      kind: "text_delta",
      session_id: "session-other",
      thread_id: "thread-1",
      turn_id: "turn-1",
      message_id: "a1",
      text: "nope",
    };
    assert.equal(isStaleChatEvent(state, staleSession), true);
    state = reduceChatEvent(state, staleSession).state;
    assert.equal(state.messages.length, 1);

    const staleTurn: ChatEvent = {
      kind: "text_delta",
      session_id: "session-1",
      thread_id: "thread-1",
      turn_id: "turn-old",
      message_id: "a1",
      text: "nope",
    };
    assert.equal(isStaleChatEvent(state, staleTurn), true);

    const { effects } = reduceChatEvent(state, {
      kind: "warning",
      session_id: "session-1",
      thread_id: "thread-1",
      message: "Chat history could not be saved.",
    });
    assert.equal(effects.warningToast, "Chat history could not be saved.");
  });

  it("cancelled ends sending and marks assistant", () => {
    const state = reduceAll([
      { kind: "user", ...ID, message_id: "u1", text: "hi" },
      { kind: "text_delta", ...ID, message_id: "a1", text: "partial" },
      { kind: "cancelled", ...ID, message_id: "a1" },
    ]);
    assert.equal(state.sending, false);
    assert.equal(assistant(state).streaming, false);
    assert.equal(assistant(state).error, "turn cancelled");
    assert.equal(state.currentTurnId, null);
  });

  it("warning does not mutate messages and allows cross-turn toast", () => {
    let state = reduceAll([
      { kind: "user", ...ID, message_id: "u1", text: "hi" },
      { kind: "text_delta", ...ID, message_id: "a1", text: "x" },
    ]);
    const before = state.messages;
    const { state: next, effects } = reduceChatEvent(state, {
      kind: "warning",
      session_id: ID.session_id,
      thread_id: ID.thread_id,
      turn_id: "other-turn",
      message: "checkpoint failed",
    });
    assert.equal(next.messages, before);
    assert.equal(effects.warningToast, "checkpoint failed");
  });

  it("restores approval card after failed resolve", () => {
    const state = reduceAll([
      {
        kind: "approval_needed",
        ...ID,
        message_id: "a1",
        approval_id: "ap1",
        tool_name: "write_file",
        tool_call_id: "t1",
        risk: "write",
        path: "f.txt",
        summary: "write f.txt",
        diff: "+x",
      },
    ]);
    const snap = findApprovalTool(state.messages, "ap1");
    assert.ok(snap);
    assert.equal(snap?.status, "awaiting_approval");

    const running = markApprovalRunning(state.messages, "ap1");
    assert.equal(assistant({ ...state, messages: running }).tools[0].status, "running");
    assert.equal(
      assistant({ ...state, messages: running }).tools[0].approvalId,
      undefined
    );

    const restored = restoreApprovalCard(running, snap!);
    const tool = assistant({ ...state, messages: restored }).tools[0];
    assert.equal(tool.status, "awaiting_approval");
    assert.equal(tool.approvalId, "ap1");
    assert.equal(tool.diff, "+x");
  });

  it("retires an approval card whose waiter the backend dropped", () => {
    const state = reduceAll([pendingApproval("ap1", "t1")]);
    const retired = retireApprovalCard(state.messages, "ap1");
    const tool = assistant({ ...state, messages: retired }).tools[0];
    assert.equal(tool.status, "error");
    // Nothing may still point at the dead waiter, or the card keeps offering
    // buttons that can only fail.
    assert.equal(tool.approvalId, undefined);
    assert.match(tool.summary ?? "", /approval expired/);
  });

  /**
   * A terminal turn cannot leave a tool waiting on a human. These rows used to
   * survive, and because the approval queue scans the whole transcript in
   * order, a dead one shadowed the live card and could never be cleared.
   */
  for (const kind of ["done", "cancelled", "error"] as const) {
    it(`${kind} terminalizes a tool still awaiting approval`, () => {
      const state = reduceAll([
        pendingApproval("ap1", "t1"),
        kind === "error"
          ? { kind, ...ID, message_id: "a1", message: "boom" }
          : { kind, ...ID, message_id: "a1" },
      ]);
      const tool = assistant(state).tools[0];
      assert.equal(tool.status, "error");
      assert.match(tool.summary ?? "", /approval interrupted/);
      assert.equal(
        state.messages
          .filter((m) => m.role === "assistant")
          .flatMap((m) => m.tools)
          .filter((t) => t.status === "awaiting_approval").length,
        0
      );
    });
  }

  it("terminalizes a stale approval left in an earlier assistant message", () => {
    // The queue is flattened across messages, so the sweep has to be too.
    const state = reduceAll([
      pendingApproval("ap1", "t1", "a1"),
      { kind: "assistant_start", ...ID, message_id: "a2" },
      { kind: "cancelled", ...ID, message_id: "a2" },
    ]);
    assert.equal(assistant(state, 0).tools[0].status, "error");
  });
});

describe("joinThinkingStream", () => {
  it("separates adjacent bold summary titles instead of fusing them", () => {
    // The reported bug: two `**Title**` blocks welded into `****`, which is not
    // valid emphasis and renders as literal asterisks.
    const joined = joinThinkingStream(
      "**Planning project inspection**",
      "**Planning React Vite with Tailwind scaffolding**"
    );
    assert.ok(!joined.includes("****"), joined);
    assert.equal(
      joined,
      "**Planning project inspection**\n\n**Planning React Vite with Tailwind scaffolding**"
    );
  });

  it("still concatenates mid-word chunks untouched", () => {
    assert.equal(joinThinkingStream("scaff", "olding"), "scaffolding");
    assert.equal(joinThinkingStream("**bo", "ld**"), "**bold**");
  });

  it("keeps the existing sentence-boundary behaviour", () => {
    assert.equal(joinThinkingStream("Done.", " Next"), "Done. Next");
    assert.equal(joinThinkingStream("done", "Next"), "done\n\nNext");
  });

  it("starts a new block when a title follows a finished sentence", () => {
    // The column bug: each summarized step ends in prose, so the next
    // `**Title**` glued onto it and rendered inline instead of as a heading —
    // leaving nothing able to tell one step from the next.
    assert.equal(
      joinThinkingStream("Working through the details.", "**Refining eligibility logic**"),
      "Working through the details.\n\n**Refining eligibility logic**"
    );
    assert.equal(
      joinThinkingStream("Next:", "**Adding validation**"),
      "Next:\n\n**Adding validation**"
    );
  });

  it("leaves genuine inline emphasis alone", () => {
    // No sentence boundary, so this is emphasis inside the current sentence.
    assert.equal(joinThinkingStream("the", "**gateway**"), "the**gateway**");
    assert.equal(joinThinkingStream("check the ", "**gateway**"), "check the **gateway**");
  });

  it("handles empty sides", () => {
    assert.equal(joinThinkingStream("", "**A**"), "**A**");
    assert.equal(joinThinkingStream("**A**", ""), "**A**");
  });
});
