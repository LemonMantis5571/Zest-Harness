import assert from "node:assert/strict";
import test from "node:test";

import {
  pickProviderFallback,
  pickReadyProvider,
} from "./providerSelection.ts";
import type { ProviderRow } from "./types.ts";

function row(
  id: string,
  statusKind: ProviderRow["statusKind"],
  selectable: boolean
): ProviderRow {
  return {
    id,
    label: id,
    method: "API key",
    statusKind,
    statusLabel: statusKind,
    detail: statusKind,
    selectable,
    canConnect: false,
    configured: selectable,
    defaultModel: "model",
    models: [],
  };
}

test("an unavailable remembered provider does not auto-switch to another one", () => {
  const rows = [
    row("codex", "ready", false),
    row("anthropic", "ready", true),
  ];

  assert.equal(pickReadyProvider(rows, null, () => false)?.id, "anthropic");
  assert.equal(pickReadyProvider(rows, "codex", () => false), null);
});

test("fallback keeps an unknown configured provider actionable", () => {
  const rows = [
    row("codex", "ready", false),
    row("local", "unknown", true),
    row("deepseek", "unconfigured", false),
  ];

  assert.equal(pickProviderFallback(rows, null)?.id, "local");
});
