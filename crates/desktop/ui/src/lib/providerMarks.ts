/**
 * Which mark identifies a provider.
 *
 * Split out of the component because this is the part with rules in it, and
 * because the test runner strips types rather than compiling JSX — a `.tsx`
 * file cannot be imported by a test at all. Drawing stays in `ProviderIcon`;
 * deciding lives here.
 *
 * Matched on the **provider id**, never a model name. Zest reaches Codex
 * through a gateway, and DeepSeek, a local Ollama server, or anything else
 * through the same `openai_compatible` shape — so the model says little about
 * who is being talked to, while the id is exactly what the user configured.
 */
export type ProviderMark = "codex" | "claude" | "deepseek" | "gemini" | "generic";

const RULES: { mark: Exclude<ProviderMark, "generic">; test: (id: string) => boolean }[] = [
  { mark: "codex", test: (id) => id === "codex" || id.startsWith("codex-") || id.startsWith("openai") },
  { mark: "claude", test: (id) => id === "claude" || id.startsWith("anthropic") },
  { mark: "deepseek", test: (id) => id.startsWith("deepseek") },
  {
    mark: "gemini",
    test: (id) => id.startsWith("gemini") || id === "antigravity",
  },
];

/**
 * The mark for a provider id.
 *
 * `generic` for anything unrecognised, which is the ordinary case for a local
 * model rather than a failure — a provider Zest has never heard of should look
 * deliberately plain, not broken.
 */
export function providerMark(providerId?: string | null): ProviderMark {
  const id = (providerId ?? "").trim().toLowerCase();
  if (!id) return "generic";
  return RULES.find((rule) => rule.test(id))?.mark ?? "generic";
}

/** Whether a provider has a mark of its own, for callers that want to say so. */
export function hasProviderMark(providerId?: string | null): boolean {
  return providerMark(providerId) !== "generic";
}
