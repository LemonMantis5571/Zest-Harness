import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { sendOwnsComposer, type SendTurnRequest } from "./sendTurn.ts";

describe("sendOwnsComposer", () => {
  it("lets a composer send clear the draft even when it carries text", () => {
    const request: SendTurnRequest = {
      origin: "composer",
      text: "Hi, whatsupp, whats this project about?",
    };
    assert.equal(sendOwnsComposer(request.origin), true);
  });

  it("leaves the composer alone for a questionnaire answer", () => {
    const request: SendTurnRequest = { origin: "answer", text: "ship it" };
    assert.equal(sendOwnsComposer(request.origin), false);
  });
});
