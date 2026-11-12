// Fence tags pass through to the highlighter, which owns the list of grammars
// (`codeBlockLanguages` in components/reui/code-block/code-block-highlight.tsx).
// Keeping a second allowlist here silently turned every language it forgot
// into plain text, even when a grammar existed.
const ALIASES: Record<string, string> = {
  ts: "typescript",
  js: "javascript",
  py: "python",
  rs: "rust",
  sh: "bash",
  zsh: "bash",
  shell: "shellscript",
  ps1: "powershell",
  pwsh: "powershell",
  kt: "kotlin",
  kts: "kotlin",
  gql: "graphql",
  yml: "yaml",
  md: "markdown",
  text: "plaintext",
  txt: "plaintext",
  plain: "plaintext",
  "": "plaintext",
};

/** A fence tag worth showing as-is; anything else renders as plain text. */
const FENCE_TAG = /^[a-z0-9][a-z0-9+#._-]{0,31}$/;

export function normalizeLang(raw: string | undefined | null): string {
  const key = (raw ?? "").trim().toLowerCase();
  const lang = ALIASES[key] ?? key;
  return FENCE_TAG.test(lang) ? lang : "plaintext";
}

export function languageLabel(lang: string): string {
  const normalized = normalizeLang(lang);
  if (normalized === "plaintext") return "text";
  if (normalized === "typescript") return "ts";
  if (normalized === "javascript") return "js";
  if (normalized === "shellscript") return "shell";
  if (normalized === "powershell") return "ps1";
  return normalized;
}
