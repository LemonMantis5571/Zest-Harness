import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  effortFromSession,
  mergeSessionOptions,
  resetSessionOptions,
  rollbackSessionOptions,
} from "./sessionOptions.ts";
import type { SessionInfo, SessionMeta } from "./types.ts";

const base: SessionInfo = {
  sessionId: "s1",
  provider: "codex",
  label: "Codex",
  model: "gpt-5.4",
  effort: "high",
  root: ".",
  isFreeChat: false,
  threadId: "t1",
  defaultModel: "gpt-5.6-sol",
  models: [
    {
      id: "gpt-5.6-sol",
      efforts: ["low", "medium", "high", "xhigh", "max"],
      contextWindow: 256000,
      supportsTools: true,
      supportsVision: false,
    },
    {
      id: "gpt-5.4",
      efforts: ["low", "medium", "high", "xhigh", "max"],
      contextWindow: 256000,
      supportsTools: true,
      supportsVision: false,
    },
  ],
  ownsAgentLoop: false,
  checkpoints: [],
  pendingInputs: [],
  messages: [{ id: "u1", role: "user", text: "hi" }],
  hasOlderMessages: false,
  hasNewerMessages: false,
  hiddenUserTurns: 0,
};

/** What Rust now replies with when only options changed: no transcript. */
const meta: SessionMeta = (() => {
  const {
    messages: _messages,
    hasOlderMessages: _hasOlder,
    hasNewerMessages: _hasNewer,
    hiddenUserTurns: _hidden,
    ...rest
  } = base;
  return rest;
})();

describe("session options authority", () => {
  it("takes the authoritative options and leaves the transcript alone", () => {
    const merged = mergeSessionOptions(base, {
      ...meta,
      model: "gpt-5.3",
      effort: "medium",
    });
    assert.ok(merged);
    assert.equal(merged?.model, "gpt-5.3");
    assert.equal(merged?.effort, "medium");
    // The messages were never in the reply; they come from the session already
    // held, which is the whole point of not shipping them.
    assert.equal(merged?.messages.length, 1);
    assert.equal(merged?.messages[0].id, "u1");
  });

  it("has nothing to update when no session is open", () => {
    // A reply about options cannot conjure a conversation that was not open.
    assert.equal(mergeSessionOptions(null, meta), null);
  });

  it("rolls back optimistic model/effort on failure", () => {
    const optimistic: SessionInfo = {
      ...base,
      model: "bad-model",
      effort: "low",
    };
    const rolled = rollbackSessionOptions(optimistic, {
      model: base.model,
      effort: "high",
    });
    assert.ok(rolled);
    assert.equal(rolled?.model, "gpt-5.4");
    assert.equal(rolled?.effort, "high");
  });

  it("maps unknown effort to fallback", () => {
    assert.equal(effortFromSession("nope", "high"), "high");
    assert.equal(effortFromSession("xhigh", "high"), "xhigh");
  });

  it("keeps the default model and effort in one reset payload", () => {
    assert.deepEqual(
      resetSessionOptions({ model: "gpt-5.6-sol", effort: "high" }),
      { model: "gpt-5.6-sol", effort: "high" }
    );
  });
});
