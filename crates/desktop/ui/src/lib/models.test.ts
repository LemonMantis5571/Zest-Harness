import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  effortsForModel,
  filterModelPickerGroups,
  formatContextWindow,
  modelPickerGroups,
  modelPickerHasChoices,
  sameModelCatalogue,
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

describe("model catalogue search", () => {
  const groups = [
    { providerId: "one", label: "Local models", current: true, models: [noEffortModel, { ...noEffortModel, id: "gpt-5.6-sol" }] },
    { providerId: "two", label: "Research", current: false, models: [noEffortModel] },
  ];
  it("returns the original catalogue for whitespace", () => {
    assert.equal(filterModelPickerGroups(groups, "  "), groups);
  });
  it("matches provider labels case insensitively and preserves the group", () => {
    assert.deepEqual(filterModelPickerGroups(groups, "LOCAL MODELS"), [groups[0]]);
  });
  it("matches display labels and model IDs without mutating the source", () => {
    assert.deepEqual(filterModelPickerGroups(groups, "5.6 sol")[0].models.map((model) => model.id), ["gpt-5.6-sol"]);
    assert.equal(groups[0].models.length, 2);
  });
  it("retains duplicate model IDs under their provider identities in stable order", () => {
    assert.deepEqual(filterModelPickerGroups(groups, "LOCAL-MODEL").map((group) => group.providerId), ["one", "two"]);
  });
  it("returns no groups when nothing matches", () => {
    assert.deepEqual(filterModelPickerGroups(groups, "not-a-model"), []);
  });
});

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

  it("lists other selectable providers after the current session", () => {
    const groups = modelPickerGroups(
      { providerId: "codex", label: "Codex", models: [noEffortModel] },
      [
        {
          id: "codex",
          label: "Codex",
          selectable: true,
          models: [noEffortModel],
          defaultModel: "local-model",
        },
        {
          id: "deepseek",
          label: "DeepSeek",
          selectable: true,
          models: [],
          defaultModel: "deepseek-v4-flash",
        },
        {
          id: "claude",
          label: "Claude",
          selectable: false,
          models: [],
          defaultModel: "sonnet",
        },
      ]
    );
    assert.deepEqual(
      groups.map((group) => group.providerId),
      ["codex", "deepseek"]
    );
    assert.equal(groups[1]?.models[0]?.id, "deepseek-v4-flash");
    assert.equal(modelPickerHasChoices(groups), true);
  });

  it("does not list a sibling that advertises the same models", () => {
    const luna: ModelCapability = { ...noEffortModel, id: "gpt-5.6-luna" };
    const sol: ModelCapability = { ...noEffortModel, id: "gpt-5.6-sol" };
    assert.equal(sameModelCatalogue([luna, sol], [sol, luna]), true);
    const groups = modelPickerGroups(
      { providerId: "codex", label: "Codex", models: [luna, sol] },
      [
        {
          id: "codex",
          label: "Codex",
          selectable: true,
          models: [luna, sol],
          defaultModel: "gpt-5.6-sol",
        },
        {
          id: "codex-chatgpt",
          label: "Codex",
          selectable: true,
          models: [sol, luna],
          defaultModel: "gpt-5.6-sol",
        },
        {
          id: "deepseek",
          label: "DeepSeek",
          selectable: true,
          models: [],
          defaultModel: "deepseek-v4-flash",
        },
      ]
    );
    assert.deepEqual(
      groups.map((group) => group.providerId),
      ["codex", "deepseek"]
    );
  });

  it("formats context capacity without pretending it is usage", () => {
    assert.equal(formatContextWindow(128_000), "128k context");
    assert.equal(formatContextWindow(1_000_000), "1.0M context");
    assert.equal(formatContextWindow(0), null);
  });
});
