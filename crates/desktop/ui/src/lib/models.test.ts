import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  effortsForModel,
  formatContextWindow,
  sessionSupportsModelPicker,
  type ModelCapability,
} from "./models.ts";

const noEffortModel: ModelCapability = {
  id: "local-model",
  efforts: [],
  contextWindow: 32_000,
  supportsTools: true,
  supportsVision: false,
};

describe("model capability helpers", () => {
  it("does not invent an effort selector for models with no effort support", () => {
    assert.deepEqual(effortsForModel([noEffortModel], noEffortModel.id), []);
  });

  it("keeps legacy capability data usable when no model is supplied", () => {
    assert.equal(effortsForModel(undefined, "gpt-5.6-sol").length, 5);
  });

  it("only shows the model picker when the provider exposes alternatives", () => {
    assert.equal(sessionSupportsModelPicker([noEffortModel]), false);
    assert.equal(
      sessionSupportsModelPicker([
        noEffortModel,
        { ...noEffortModel, id: "another-model" },
      ]),
      true
    );
  });

  it("formats context capacity without pretending it is usage", () => {
    assert.equal(formatContextWindow(128_000), "128k context");
    assert.equal(formatContextWindow(1_000_000), "1.0M context");
    assert.equal(formatContextWindow(0), null);
  });
});
