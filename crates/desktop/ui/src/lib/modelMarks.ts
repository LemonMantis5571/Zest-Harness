/**
 * Which mark identifies a model family.
 *
 * This intentionally lives apart from provider marks. A provider tells us
 * where a request is routed; a model mark tells us what the user selected.
 * The same model can appear under more than one provider.
 */
export type ModelMark =
  | "sol"
  | "terra"
  | "luna"
  | "mini"
  | "codex"
  | "cursor"
  | "claude"
  | "gemini"
  | "deepseek"
  | "llama"
  | "standard"
  | "generic";

const RULES: {
  mark: Exclude<ModelMark, "generic">;
  test: (id: string) => boolean;
}[] = [
  { mark: "sol", test: (id) => id === "sol" || id.endsWith("-sol") },
  { mark: "terra", test: (id) => id === "terra" || id.endsWith("-terra") },
  { mark: "luna", test: (id) => id === "luna" || id.endsWith("-luna") },
  {
    mark: "codex",
    test: (id) => id === "codex" || id.startsWith("codex-") || id.includes("/codex-"),
  },
  {
    mark: "cursor",
    test: (id) => id === "cursor" || id.startsWith("cursor-") || id.includes("/cursor-"),
  },
  { mark: "mini", test: (id) => id.includes("-mini") || id.endsWith("mini") },
  {
    mark: "claude",
    test: (id) =>
      id.startsWith("claude") ||
      id.includes("sonnet") ||
      id.includes("opus") ||
      id.includes("haiku"),
  },
  { mark: "gemini", test: (id) => id.startsWith("gemini") },
  { mark: "deepseek", test: (id) => id.startsWith("deepseek") },
  {
    mark: "llama",
    test: (id) => id.startsWith("llama") || id.startsWith("meta-llama"),
  },
  {
    mark: "standard",
    test: (id) =>
      id.startsWith("gpt-") ||
      id.startsWith("o1") ||
      id.startsWith("o3") ||
      id.startsWith("o4"),
  },
];

/** Return a stable visual mark for a model id, with a plain fallback. */
export function modelMark(modelId?: string | null): ModelMark {
  const id = (modelId ?? "").trim().toLowerCase();
  if (!id) return "generic";
  return RULES.find((rule) => rule.test(id))?.mark ?? "generic";
}
