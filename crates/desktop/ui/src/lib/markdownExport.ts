const WINDOWS_RESERVED_NAMES = new Set([
  "CON",
  "PRN",
  "AUX",
  "NUL",
  ...Array.from({ length: 9 }, (_, index) => `COM${index + 1}`),
  ...Array.from({ length: 9 }, (_, index) => `LPT${index + 1}`),
]);

/** Turn user/model text into a safe, readable Markdown filename. */
export function safeMarkdownFilename(value: string, fallback = "response") {
  const withoutExtension = value.trim().replace(/\.md$/i, "");
  const safe = Array.from(withoutExtension, (character) =>
    character.charCodeAt(0) < 32 || /[<>:"/\\|?*]/.test(character)
      ? "-"
      : character
  )
    .join("")
    .replace(/\s+/g, " ")
    .replace(/[. ]+$/g, "")
    .trim();
  const stem = safe || fallback;
  const guarded = WINDOWS_RESERVED_NAMES.has(stem.toUpperCase()) ? `_${stem}` : stem;
  return `${guarded.slice(0, 120)}.md`;
}

/** Use the first Markdown heading as the natural filename for a reply. */
export function suggestedMarkdownFilename(markdown: string) {
  const heading =
    markdown.match(/^\s{0,3}#{1,6}[ \t]+([^\r\n]+?)[ \t]*#*[ \t]*$/m)?.[1] ?? "";
  return safeMarkdownFilename(heading, "response");
}

export function commandMarkdownFilename(command: string) {
  return safeMarkdownFilename(command, "response");
}
