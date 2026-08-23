import type { ChatEvent } from "./types.ts";

/** Offline UI streaming demo — no gateway required. */
export async function runFixtureStream(
  onEvent: (event: ChatEvent) => void
): Promise<void> {
  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
  const sessionId = "session-fixture";
  const threadId = "fixture";
  const turnId = "turn-fixture";
  const userId = "user-fixture";
  const assistantId = "assistant-fixture";

  const id = {
    session_id: sessionId,
    thread_id: threadId,
    turn_id: turnId,
  };

  // Keep one other chat visibly busy long enough to exercise the sidebar
  // activity mark offline. It is deliberately a local-provider thread so the
  // same fixture covers both the branded and generic provider marks.
  const background = {
    session_id: sessionId,
    thread_id: "fixture-local",
    turn_id: "turn-fixture-background",
    message_id: "assistant-fixture-background",
  };
  onEvent({
    kind: "user",
    ...background,
    message_id: "user-fixture-background",
    text: "Check the local workspace status.",
  });
  onEvent({ kind: "assistant_start", ...background });
  onEvent({
    kind: "tool_call_start",
    ...background,
    name: "git_status",
    id: "tool_fixture_background",
  });
  globalThis.setTimeout(() => {
    onEvent({
      kind: "tool_call_result",
      ...background,
      name: "git_status",
      id: "tool_fixture_background",
      summary: "1 changed file",
      isError: false,
    });
    onEvent({ kind: "done", ...background });
  }, 9_000);

  onEvent({
    kind: "user",
    ...id,
    message_id: userId,
    text: "What's in README.md?",
  });
  await sleep(120);

  onEvent({
    kind: "assistant_start",
    ...id,
    message_id: assistantId,
  });
  await sleep(200);

  onEvent({
    kind: "thinking_delta",
    ...id,
    message_id: assistantId,
    text: "I'll read the project README first.",
  });
  await sleep(350);

  // Several reads rather than one, so the offline fixture exercises the run
  // grouping: they stay as separate rows while the turn works, then fold into a
  // single summary once the last one settles.
  const reads = [
    { id: "tool_fixture_1", summary: "# Zest — Rust coding harness…" },
    { id: "tool_fixture_2", summary: "[workspace] resolver = \"2\"…" },
    { id: "tool_fixture_3", summary: "# Project Context — **Zest**…" },
    { id: "tool_fixture_4", summary: "# Recurring Corrections…" },
  ];
  for (const read of reads) {
    onEvent({
      kind: "tool_call_start",
      ...id,
      message_id: assistantId,
      name: "read_file",
      id: read.id,
    });
    await sleep(180);
    onEvent({
      kind: "tool_call_result",
      ...id,
      message_id: assistantId,
      name: "read_file",
      id: read.id,
      summary: read.summary,
      isError: false,
    });
    await sleep(120);
  }
  await sleep(200);

  const reply =
    "Zest is a Rust coding harness with a Tauri desktop shell. The chat UI streams tool calls and text over Tauri events.";
  for (const word of reply.split(/(?<=\s)/)) {
    onEvent({
      kind: "text_delta",
      ...id,
      message_id: assistantId,
      text: word,
    });
    await sleep(28);
  }

  onEvent({ kind: "done", ...id, message_id: assistantId });
}
