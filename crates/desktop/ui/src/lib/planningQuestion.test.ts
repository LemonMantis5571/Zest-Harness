import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { planningQuestionFor } from "./planningQuestion.ts";
import type { ChatMessage } from "./types.ts";

function plan(text: string, streaming = false): ChatMessage {
  return {
    id: "planning-question",
    role: "assistant",
    text,
    thinking: "",
    tools: [],
    command: "plan",
    streaming,
  };
}

describe("planningQuestionFor", () => {
  it("turns a short choice question into choices", () => {
    assert.deepEqual(
      planningQuestionFor(
        plan("Which frontend should we use?\n\n- React\n- Svelte")
      ),
      {
        prompt: "Which frontend should we use?",
        choices: [
          { value: "React", label: "React" },
          { value: "Svelte", label: "Svelte" },
        ],
      }
    );
  });

  it("supports numbered options and keeps the prompt readable", () => {
    const question = planningQuestionFor(
      plan("**How should we handle auth:**\n\n1. Keep the current flow\n2. Add a provider picker")
    );
    assert.equal(question?.prompt, "How should we handle auth:");
    assert.deepEqual(question?.choices.map((choice) => choice.label), [
      "Keep the current flow",
      "Add a provider picker",
    ]);
  });

  it("uses a text answer when no choices are supplied", () => {
    assert.deepEqual(
      planningQuestionFor(plan("What should the first release prioritize?")),
      {
        prompt: "What should the first release prioritize?",
        choices: [],
      }
    );
  });

  it("does not turn a plan document or streaming text into a questionnaire", () => {
    assert.equal(
      planningQuestionFor(plan("## Plan\n\n1. Build the shell\n2. Add tests\n\nWhich next?")),
      null
    );
    assert.equal(planningQuestionFor(plan("Which model?", true)), null);
    assert.equal(
      planningQuestionFor({
        ...plan("Which model?"),
        command: undefined,
      }),
      null
    );
  });
});
