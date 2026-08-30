export type EffortId = "low" | "medium" | "high" | "xhigh" | "max";

export type ModelOption = {
  id: string;
  label: string;
  shortLabel: string;
};

export type EffortOption = {
  id: EffortId;
  label: string;
  shortLabel: string;
};

/** Display labels only — availability comes from Rust `ProviderView` / `SessionInfo.models`. */
const MODEL_LABELS: Record<string, { label: string; shortLabel: string }> = {
  "gpt-5.6-sol": { label: "5.6 Sol", shortLabel: "Sol" },
  "gpt-5.6-terra": { label: "5.6 Terra", shortLabel: "Terra" },
  "gpt-5.6-luna": { label: "5.6 Luna", shortLabel: "Luna" },
  "gpt-5.5": { label: "5.5", shortLabel: "5.5" },
  "gpt-5.4": { label: "5.4", shortLabel: "5.4" },
  "gpt-5.4-mini": { label: "5.4 Mini", shortLabel: "Mini" },
};

export const EFFORTS: EffortOption[] = [
  { id: "low", label: "Low", shortLabel: "Low" },
  { id: "medium", label: "Medium", shortLabel: "Med" },
  { id: "high", label: "High", shortLabel: "High" },
  { id: "xhigh", label: "Extra high", shortLabel: "XHigh" },
  { id: "max", label: "Max", shortLabel: "Max" },
];

export const DEFAULT_CODEX_MODEL = "gpt-5.6-sol";
export const DEFAULT_EFFORT: EffortId = "high";

export type ModelCapability = {
  id: string;
  efforts: string[];
  contextWindow: number;
  supportsTools: boolean;
  supportsVision: boolean;
};

export function capabilityForModel(
  models: ModelCapability[] | undefined,
  modelId: string
): ModelCapability | undefined {
  return models?.find((item) => item.id === modelId);
}

export function formatContextWindow(tokens: number | undefined): string | null {
  if (!tokens || !Number.isFinite(tokens) || tokens <= 0) return null;
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}M context`;
  return `${Math.round(tokens / 1_000)}k context`;
}

export function modelLabel(modelId: string): string {
  return MODEL_LABELS[modelId]?.label ?? modelId;
}

export function modelShort(modelId: string): string {
  return MODEL_LABELS[modelId]?.shortLabel ?? modelId;
}

export function effortLabel(effortId: string): string {
  return EFFORTS.find((e) => e.id === effortId)?.label ?? effortId;
}

export function effortShort(effortId: string): string {
  return EFFORTS.find((e) => e.id === effortId)?.shortLabel ?? effortId;
}

export function chipLabel(modelId: string, effortId: string): string {
  return `${modelLabel(modelId)} · ${effortShort(effortId)}`;
}

export function isEffortId(value: string): value is EffortId {
  return EFFORTS.some((e) => e.id === value);
}

/** True when Rust exposed more than one selectable model for the session. */
export function sessionSupportsModelPicker(
  models: ModelCapability[] | undefined
): boolean {
  return (models?.length ?? 0) > 1;
}

export type ModelPickerGroup = {
  providerId: string;
  label: string;
  current: boolean;
  models: ModelCapability[];
};

const fallbackCapability = (id: string): ModelCapability => ({
  id,
  efforts: [],
  contextWindow: 0,
  supportsTools: true,
  supportsVision: false,
});

function modelsForRow(row: {
  models: ModelCapability[];
  defaultModel: string;
}): ModelCapability[] {
  if (row.models.length) return row.models;
  return row.defaultModel ? [fallbackCapability(row.defaultModel)] : [];
}

/** True when two catalogues name the same models, ignoring order. */
export function sameModelCatalogue(
  left: readonly ModelCapability[],
  right: readonly ModelCapability[]
): boolean {
  if (left.length !== right.length) return false;
  const ids = new Set(left.map((model) => model.id));
  return right.every((model) => ids.has(model.id));
}

/** Current session models first, then every other selectable provider. */
export function modelPickerGroups(
  current: {
    providerId: string;
    label: string;
    models: ModelCapability[] | undefined;
  },
  providers: ReadonlyArray<{
    id: string;
    label: string;
    selectable: boolean;
    models: ModelCapability[];
    defaultModel: string;
  }>
): ModelPickerGroup[] {
  const currentModels = current.models ?? [];
  const groups: ModelPickerGroup[] = [
    {
      providerId: current.providerId,
      label: current.label,
      current: true,
      models: currentModels,
    },
  ];
  for (const row of providers) {
    if (!row.selectable || row.id === current.providerId) continue;
    const models = modelsForRow(row);
    if (models.length === 0) continue;
    // ChatGPT Codex and Codex CLI advertise the same list. Showing both
    // stacks the same six models twice; the provider sheet is how you
    // change the transport.
    if (sameModelCatalogue(currentModels, models)) continue;
    groups.push({
      providerId: row.id,
      label: row.label,
      current: false,
      models,
    });
  }
  return groups;
}

export function modelPickerHasChoices(groups: ModelPickerGroup[]): boolean {
  const models = groups.reduce((count, group) => count + group.models.length, 0);
  return groups.length > 1 || models > 1;
}

/** Map Rust catalogue + display labels for the picker. */
export function modelOptionsFromCapabilities(
  models: ModelCapability[] | undefined
): ModelOption[] {
  if (!models?.length) {
    return [
      {
        id: DEFAULT_CODEX_MODEL,
        label: modelLabel(DEFAULT_CODEX_MODEL),
        shortLabel: modelShort(DEFAULT_CODEX_MODEL),
      },
    ];
  }
  return models.map((m) => ({
    id: m.id,
    label: modelLabel(m.id),
    shortLabel: modelShort(m.id),
  }));
}

export function effortsForModel(
  models: ModelCapability[] | undefined,
  modelId: string
): EffortOption[] {
  const spec = models?.find((m) => m.id === modelId);
  if (models?.length) {
    // An explicitly empty capability list means this model has no effort
    // control. Unknown/legacy model data keeps the historical fallback.
    if (spec && spec.efforts.length === 0) return [];
  }
  const allowed = spec?.efforts?.length
    ? spec.efforts
    : EFFORTS.map((e) => e.id);
  return EFFORTS.filter((e) => allowed.includes(e.id));
}

/** @deprecated Use sessionSupportsModelPicker(session.models). */
export function providerSupportsModelPicker(provider: string): boolean {
  return provider === "codex" || provider === "fixture";
}

/** Legacy constant for fixture defaults — not an availability source. */
export const CODEX_MODELS: ModelOption[] = Object.entries(MODEL_LABELS).map(
  ([id, labels]) => ({ id, ...labels })
);
