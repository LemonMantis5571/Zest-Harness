import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  CODEX_MODELS,
  catalogueModelLabel,
  cliRequirementDetail,
  cliRequirementLabel,
  effortsForModel,
  filterModelPickerGroups,
  formatContextWindow,
  modelOptionsFromCapabilities,
  modelPickerGroups,
  modelPickerHasChoices,
  providerSource,
  sameModelCatalogue,
  sessionSupportsModelPicker,
  sharedProviderLabels,
  type ModelCapability,
} from "./models.ts";

const noEffortModel: ModelCapability = {
  id: "local-model",
  efforts: [],
  contextWindow: 32_000,
  supportsTools: true,
  supportsVision: false,
};

describe("current model display names", () => {
  it("labels backend choices without adding other providers to the Codex fixture", () => {
    const ids = ["gpt-6.1-sol", "gpt-6-astra", "claude-sonnet-5-5", "claude-haiku-5-5", "claude-fable-5-1", "composer-2.5", "grok-4.7"];
    const options = modelOptionsFromCapabilities(ids.map((id) => ({ ...noEffortModel, id })));
    assert.deepEqual(options.map((model) => model.label), ["6.1 Sol", "6 Astra", "Sonnet 5.5", "Haiku 5.5", "Fable 5.1", "Composer 2.5", "Grok 4.7"]);
    assert.deepEqual(options.map((model) => model.id), ids);
    assert.equal(CODEX_MODELS.every((model) => model.id.startsWith("gpt-")), true);
  });
});

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

  it("leaves out providers the user hid, but never the current one", () => {
    const providers = [
      { id: "codex", label: "Codex", selectable: true, models: [noEffortModel], defaultModel: "local-model" },
      { id: "deepseek", label: "DeepSeek", selectable: true, models: [], defaultModel: "deepseek-v4-flash" },
      { id: "cursor", label: "Cursor", selectable: true, models: [], defaultModel: "composer-2.5" },
    ];
    const groups = modelPickerGroups(
      { providerId: "codex", label: "Codex", models: [noEffortModel] },
      providers,
      new Set(["codex", "deepseek"])
    );
    assert.deepEqual(
      groups.map((group) => group.providerId),
      ["codex", "cursor"]
    );
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

describe("catalogueModelLabel", () => {
  const capability = (id: string, resolvesTo?: string): ModelCapability => {
    const model: ModelCapability = {
      id,
      efforts: [],
      contextWindow: 0,
      supportsTools: true,
      supportsVision: false,
    };
    if (resolvesTo) model.resolvesTo = resolvesTo;
    return model;
  };

  it("names what a resolved alias actually runs", () => {
    const models = [capability("opus", "claude-opus-5"), capability("haiku", "claude-haiku-4-5-20251001")];
    assert.equal(catalogueModelLabel(models, "opus"), "opus → Opus 5");
    assert.equal(catalogueModelLabel(models, "haiku"), "haiku → Haiku 4.5");
  });

  it("marks an alias no turn has resolved yet", () => {
    assert.equal(catalogueModelLabel([capability("opus")], "opus"), "opus (alias)");
  });

  it("leaves explicit and unknown models as they were", () => {
    const models = [capability("claude-opus-5-5")];
    assert.equal(catalogueModelLabel(models, "claude-opus-5-5"), "Opus 5.5");
    assert.equal(catalogueModelLabel(undefined, "some-local-model"), "some-local-model");
  });

  it("lets search find an alias by the model it resolves to", () => {
    const groups = [
      { providerId: "claude", label: "Claude", current: true, models: [capability("opus", "claude-opus-5"), capability("claude-opus-5-5")] },
    ];
    const found = filterModelPickerGroups(groups, "opus 5");
    assert.deepEqual(found[0].models.map((model) => model.id), ["opus", "claude-opus-5-5"]);
    assert.deepEqual(filterModelPickerGroups(groups, "opus 5.5")[0].models.map((model) => model.id), ["claude-opus-5-5"]);
  });
});

describe("models the installed CLI is too old for", () => {
  const opus: ModelCapability = {
    id: "claude-opus-5-5",
    efforts: ["low", "high"],
    contextWindow: 1_000_000,
    supportsTools: true,
    supportsVision: false,
    needsCli: { cli: "Claude Code", minimum: "2.1.280", installed: "2.1.220", update: "claude update" },
  };
  const sonnet: ModelCapability = { ...opus, id: "claude-sonnet-5-5", needsCli: undefined };

  it("says which CLI version a marked model needs and how to get it", () => {
    assert.equal(cliRequirementLabel(opus), "Needs Claude Code 2.1.280+ · run claude update");
    assert.equal(
      cliRequirementDetail(opus),
      "Claude Code 2.1.220 is installed. This model needs 2.1.280 or newer, so its turns may be refused. Run claude update, then restart Zest."
    );
  });

  it("says nothing about a model with no known requirement", () => {
    assert.equal(cliRequirementLabel(sonnet), null);
    assert.equal(cliRequirementDetail(sonnet), null);
    assert.equal(cliRequirementLabel(undefined), null);
  });

  it("keeps a marked model selectable and searchable", () => {
    const groups = [{ providerId: "claude", label: "Claude Code", current: true, models: [opus, sonnet] }];
    assert.deepEqual(filterModelPickerGroups(groups, "opus")[0].models.map((model) => model.id), ["claude-opus-5-5"]);
    assert.deepEqual(effortsForModel(groups[0].models, "claude-opus-5-5").map((effort) => effort.id), ["low", "high"]);
  });
});

describe("provider source", () => {
  it("calls a provider that owns its agent loop a CLI and anything else an API", () => {
    assert.equal(providerSource(true), "cli");
    assert.equal(providerSource(false), "api");
    assert.equal(providerSource(undefined), "api");
  });

  it("finds the labels two providers share", () => {
    const shared = sharedProviderLabels([
      { label: "Codex" },
      { label: "Claude" },
      { label: "Codex" },
      { label: "Cursor" },
    ]);
    assert.deepEqual([...shared], ["Codex"]);
  });

  it("carries each group's source and connection method", () => {
    const groups = modelPickerGroups(
      { providerId: "codex", label: "Codex", models: [noEffortModel] },
      [
        { id: "codex", label: "Codex", selectable: true, models: [noEffortModel], defaultModel: "local-model", ownsAgentLoop: false, method: "API key" },
        { id: "codex-chatgpt", label: "Codex", selectable: true, models: [], defaultModel: "gpt-6-sol", ownsAgentLoop: false, method: "ChatGPT sign-in" },
        { id: "claude-code", label: "Claude", selectable: true, models: [], defaultModel: "claude-sonnet-5-5", ownsAgentLoop: true, method: "Claude Code subscription" },
      ]
    );
    assert.deepEqual(
      groups.map((group) => [group.providerId, group.source, group.method]),
      [
        ["codex", "api", "API key"],
        ["codex-chatgpt", "api", "ChatGPT sign-in"],
        ["claude-code", "cli", "Claude Code subscription"],
      ]
    );
  });
});
