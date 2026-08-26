import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { buildablePlanId } from "./planActions.ts";
import type { ChatMessage } from "./types.ts";

const PLAN_TEXT = "## Plan\n\n1. Fix the lint setup\n2. Add git";

function plan(id: string, over = true): ChatMessage {
  return {
    id,
    role: "assistant",
    text: PLAN_TEXT,
    thinking: "",
    tools: [],
    command: "plan",
    streaming: !over,
  };
}

function reply(id: string, text: string): ChatMessage {
  return { id, role: "assistant", text, thinking: "", tools: [], streaming: false };
}

function asked(id: string, text: string): ChatMessage {
  return { id, role: "user", text };
}

describe("buildablePlanId", () => {
  it("offers Build on a finished plan", () => {
    assert.equal(buildablePlanId([asked("u1", "plan it"), plan("a1")]), "a1");
  });

  it("offers nothing while the plan is still streaming", () => {
    assert.equal(buildablePlanId([asked("u1", "plan it"), plan("a1", false)]), null);
  });

  it("offers nothing when there is no plan", () => {
    assert.equal(buildablePlanId([asked("u1", "hi"), reply("a1", "hello")]), null);
    assert.equal(buildablePlanId([]), null);
  });

  it("offers only the newest plan", () => {
    const id = buildablePlanId([plan("a1"), asked("u2", "redo it"), plan("a2")]);
    assert.equal(id, "a2", "an older plan must not be buildable");
  });

  it("withdraws the button once the conversation moves past the plan", () => {
    const after = [plan("a1"), asked("u2", "what about tests?"), reply("a2", "Sure.")];
    assert.equal(buildablePlanId(after), null);
  });

  it("does not count a silent tool turn as moving on", () => {
    // The plan is written across turns; a tool-only turn is how it got written.
    const tooled: ChatMessage = {
      id: "a2",
      role: "assistant",
      text: "",
      thinking: "",
      tools: [],
      streaming: false,
    };
    assert.equal(buildablePlanId([plan("a1"), tooled]), "a1");
  });

  it("does not offer Build on a plan-tagged clarifying question", () => {
    // Plan mode tags every turn, so a short question carries command="plan"
    // without being a plan. Same predicate the card uses.
    const question: ChatMessage = {
      id: "a1",
      role: "assistant",
      text: "Which framework?",
      thinking: "",
      tools: [],
      command: "plan",
      streaming: false,
    };
    assert.equal(buildablePlanId([question]), null);
  });

  it("does not offer Build on a numbered choice question", () => {
    const question: ChatMessage = {
      id: "a1",
      role: "assistant",
      text: "Which framework should we use?\n\n1. React\n2. Svelte",
      thinking: "",
      tools: [],
      command: "plan",
      streaming: false,
    };
    assert.equal(buildablePlanId([question]), null);
  });
});
