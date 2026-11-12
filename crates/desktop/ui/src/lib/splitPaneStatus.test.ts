import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { paneStripLabel, splitPaneStatus } from "./splitPaneStatus.ts";

describe("split pane status", () => {
  it("puts a chat waiting on you ahead of everything else", () => {
    const approval = splitPaneStatus({
      hasChat: true,
      activity: { state: "awaiting_approval" },
      sending: true,
      unread: true,
    });
    const question = splitPaneStatus({ hasChat: true, activity: { state: "awaiting_input" } });

    assert.deepEqual(approval, { kind: "needs_input", label: "Awaiting approval" });
    assert.deepEqual(question, { kind: "needs_input", label: "Waiting for your answer" });
  });

  it("reports a running turn from either the activity map or the pane's own flag", () => {
    assert.equal(splitPaneStatus({ hasChat: true, activity: { state: "working" } }).kind, "working");
    assert.equal(splitPaneStatus({ hasChat: true, sending: true }).kind, "working");
  });

  it("marks a finished chat unread only when nothing is running", () => {
    assert.equal(splitPaneStatus({ hasChat: true, unread: true }).kind, "unread");
    assert.equal(
      splitPaneStatus({ hasChat: true, activity: { state: "working" }, unread: true }).kind,
      "working"
    );
    assert.equal(splitPaneStatus({ hasChat: true, activity: { state: "idle" } }).kind, "idle");
  });

  it("names a pane without a chat as empty", () => {
    assert.equal(splitPaneStatus({ hasChat: false, unread: true }).kind, "empty");
  });

  it("builds the strip's accessible name from the action and the status", () => {
    const status = splitPaneStatus({ hasChat: true, activity: { state: "awaiting_approval" } });

    assert.equal(paneStripLabel("Fix login", status), "Restore Fix login, awaiting approval");
  });
});
