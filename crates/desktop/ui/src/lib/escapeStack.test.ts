import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { escapeAction } from "./escapeStack.ts";

const none = {
  diff: false,
  providerSwitch: false,
  modelPicker: false,
  settings: false,
  palette: false,
  editing: false,
  shellPanel: false,
  sending: false,
};

describe("escapeAction", () => {
  it("does nothing when the transcript is idle", () => {
    assert.equal(escapeAction(none), null);
  });

  it("closes Customize before it can cancel a running turn", () => {
    assert.equal(
      escapeAction({ ...none, shellPanel: true, sending: true }),
      "shell-panel"
    );
  });

  it("closes the model picker before it can cancel a running turn", () => {
    assert.equal(
      escapeAction({ ...none, modelPicker: true, sending: true }),
      "model-picker"
    );
  });

  it("still stops the turn from the transcript", () => {
    assert.equal(escapeAction({ ...none, sending: true }), "stop-turn");
  });

  it("dismisses the top overlay first", () => {
    assert.equal(
      escapeAction({
        ...none,
        diff: true,
        settings: true,
        shellPanel: true,
        sending: true,
      }),
      "diff"
    );
  });
});
