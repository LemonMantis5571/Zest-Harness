import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { compactChatMark, foreignProvider } from "./compactChatRow.ts";

describe("compactChatMark", () => {
  it("puts a chat that needs approval ahead of everything else", () => {
    assert.equal(
      compactChatMark({
        activity: { state: "awaiting_approval" },
        hasPullRequest: true,
        branchChanged: true,
      }),
      "needs_input"
    );
  });

  it("marks a chat waiting on an answer the same way as one waiting on approval", () => {
    assert.equal(
      compactChatMark({
        activity: { state: "awaiting_input" },
        hasPullRequest: true,
        branchChanged: false,
      }),
      "needs_input"
    );
  });

  it("shows work in progress over git state", () => {
    assert.equal(
      compactChatMark({ activity: { state: "working" }, hasPullRequest: true, branchChanged: true }),
      "working"
    );
  });

  it("prefers a pull request to a changed branch once the chat is idle", () => {
    assert.equal(
      compactChatMark({ activity: { state: "idle" }, hasPullRequest: true, branchChanged: true }),
      "pull_request"
    );
    assert.equal(
      compactChatMark({ hasPullRequest: false, branchChanged: true }),
      "branch"
    );
  });

  it("falls back to idle when nothing is happening", () => {
    assert.equal(compactChatMark({ hasPullRequest: false, branchChanged: false }), "idle");
  });
});

describe("foreignProvider", () => {
  it("hides the provider a chat shares with the current session", () => {
    assert.equal(foreignProvider("deepseek", "deepseek"), null);
  });

  it("names a different provider", () => {
    assert.equal(foreignProvider("anthropic", "deepseek"), "anthropic");
  });

  it("names nothing for a chat with no recorded provider", () => {
    assert.equal(foreignProvider(undefined, "deepseek"), null);
    assert.equal(foreignProvider(null, "deepseek"), null);
  });
});
