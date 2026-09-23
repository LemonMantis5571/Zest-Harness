import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { changeReviewContext, completedPlanContext, latestFinishedPlanContext } from "./jevReviewContext.ts";
import type { ChatMessage } from "./types.ts";

const user = (id: string, text: string): ChatMessage => ({ id, role: "user", text });
const assistant = (id: string, text: string, streaming = false): ChatMessage => ({ id, role: "assistant", text, thinking: "", tools: [], streaming });

describe("Jev review inputs", () => {
  it("waits for a finished, buildable plan", () => {
    const draft = { ...assistant("p1", "## Plan\n\n1. Add an endpoint\n2. Test it", true), command: "plan" };
    assert.equal(completedPlanContext([user("u1", "Add an endpoint"), draft]), null);
    const ready = completedPlanContext([user("u1", "Add an endpoint"), { ...draft, streaming: false }]);
    assert.deepEqual(ready, { id: "p1", request: "Add an endpoint", plan: draft.text });
    assert.equal(completedPlanContext([user("u1", "Add an endpoint"), { ...draft, streaming: false }, user("u2", "Change the scope")]), null);
    assert.equal(latestFinishedPlanContext([user("u1", "Add an endpoint"), { ...draft, streaming: false }, user("u2", "Change the scope")])?.id, "p1");
  });

  it("uses the latest task and settled answer for a change review", () => {
    const context = changeReviewContext([user("u1", "Add an endpoint"), assistant("a1", "Done"), user("u2", "Add tests"), assistant("a2", "Running tests", true)]);
    assert.deepEqual(context, { objective: "Add an endpoint", claimedSummary: "Done" });
    assert.deepEqual(changeReviewContext([user("u1", "Add an endpoint"), assistant("a1", "Done"), user("u2", "Add tests"), assistant("a2", "Tests pass")]), {
      objective: "Add an endpoint\n\nAdd tests", claimedSummary: "Tests pass",
    });
    assert.equal(changeReviewContext([user("u1", "Make a plan"), { ...assistant("p1", "## Plan\n\n1. Do it"), command: "plan" }]), null);
  });
});
