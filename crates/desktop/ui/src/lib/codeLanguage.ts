const LANGS = [
  "typescript",
  "tsx",
  "javascript",
  "jsx",
  "python",
  "rust",
  "go",
  "java",
  "c",
  "cpp",
  "csharp",
  "json",
  "toml",
  "yaml",
  "markdown",
  "html",
  "css",
  "scss",
  "sql",
  "bash",
  "shellscript",
  "powershell",
  "diff",
  "plaintext",
] as const;

type Lang = (typeof LANGS)[number];

const ALIASES: Record<string, Lang> = {
  ts: "typescript",
  js: "javascript",
  py: "python",
  rs: "rust",
  sh: "bash",
  zsh: "bash",
  shell: "shellscript",
  ps1: "powershell",
  yml: "yaml",
  md: "markdown",
  text: "plaintext",
  txt: "plaintext",
  "": "plaintext",
};

export function normalizeLang(raw: string | undefined | null): Lang {
  const key = (raw ?? "").trim().toLowerCase();
  if ((LANGS as readonly string[]).includes(key)) return key as Lang;
  return ALIASES[key] ?? "plaintext";
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
