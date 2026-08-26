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
