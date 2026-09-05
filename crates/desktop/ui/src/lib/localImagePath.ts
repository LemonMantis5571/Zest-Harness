/**
 * Local raster paths that may appear in assistant markdown as a code span,
 * a bare Windows/Unix path, or a file:// URL. Claude Code often writes the
 * path and claims the image is "inline"; Zest has to turn that into an img.
 */

export type LocalImagePath = string & { readonly __brand: "LocalImagePath" };

const IMAGE_EXT = /(?:png|jpe?g|gif|webp|avif|bmp)$/i;
const EXT_SOURCE = "png|jpe?g|gif|webp|avif|bmp";

const WINDOWS_PATH = new RegExp(
  String.raw`[A-Za-z]:(?:\\|/(?!/))[^\s\`'"<>|*?]+?\.(?:${EXT_SOURCE})(?=$|[\s\`'"<>)|,;]|—|–)`,
  "gi"
);
const UNIX_PATH = new RegExp(
  String.raw`(?:^|[\s\`'"<(])(/[^\s\`'"<>|*?]+?\.(?:${EXT_SOURCE}))(?=$|[\s\`'"<>)|,;]|—|–)`,
  "gi"
);
const FILE_URL = new RegExp(
  String.raw`file:///[^\s\`'"<>]+?\.(?:${EXT_SOURCE})(?:\?[^\s\`'"]*)?(?=$|[\s\`'"<>)|,;]|—|–)`,
  "gi"
);
const MARKDOWN_IMAGE = /!\[([^\]]*)\]\(([^)]+)\)/g;
const FENCE = /^\s*```/;

function unwrap(value: string): string {
  let text = value.trim();
  if (
    (text.startsWith("`") && text.endsWith("`")) ||
    (text.startsWith("\"") && text.endsWith("\"")) ||
    (text.startsWith("'") && text.endsWith("'"))
  ) {
    text = text.slice(1, -1).trim();
  }
  return text;
}

function hasControlChars(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    if (value.charCodeAt(index) < 32) return true;
  }
  return false;
}

function isAbsoluteLocal(value: string): boolean {
  if (value.startsWith("/")) return true;
  // `https://` looks like a drive (`s:`) plus a slash. Require a real path.
  return /^[A-Za-z]:(?:\\|\/(?!\/))/.test(value);
}

function finish(value: string): LocalImagePath | null {
  const normalized = value.replace(/\\/g, "/");
  if (!normalized || hasControlChars(normalized)) return null;
  if (normalized.includes("..")) return null;
  if (normalized.startsWith("//")) return null;
  if (!IMAGE_EXT.test(normalized)) return null;
  if (!isAbsoluteLocal(normalized)) return null;
  return normalized as LocalImagePath;
}

function fromFileUrl(raw: string): LocalImagePath | null {
  try {
    const url = new URL(raw);
    if (url.protocol !== "file:") return null;
    let pathname = decodeURIComponent(url.pathname);
    if (/^\/[A-Za-z]:\//.test(pathname)) {
      pathname = pathname.slice(1);
    }
    return finish(pathname);
  } catch {
    return null;
  }
}

/** Validate one candidate. Null unless it is an absolute local raster file. */
export function parseLocalImagePath(raw: string): LocalImagePath | null {
  const text = unwrap(raw);
  if (!text) return null;
  if (text.toLowerCase().startsWith("file:")) {
    return fromFileUrl(text);
  }
  return finish(text);
}

function pushUnique(found: LocalImagePath[], candidate: string) {
  const path = parseLocalImagePath(candidate);
  if (!path) return;
  if (found.some((existing) => existing === path)) return;
  found.push(path);
}

/** Absolute local raster paths mentioned in a line of prose. */
export function localImagePathsIn(text: string): LocalImagePath[] {
  const found: LocalImagePath[] = [];
  const windows = text.matchAll(WINDOWS_PATH);
  for (const match of windows) {
    pushUnique(found, match[0]);
  }
  const unix = text.matchAll(UNIX_PATH);
  for (const match of unix) {
    pushUnique(found, match[1] ?? match[0]);
  }
  const files = text.matchAll(FILE_URL);
  for (const match of files) {
    pushUnique(found, match[0]);
  }
  return found;
}

export function localImageBasename(path: LocalImagePath): string {
  const parts = path.split("/");
  return parts[parts.length - 1] ?? path;
}

export function toFileImageUrl(path: LocalImagePath): string {
  if (/^[A-Za-z]:\//.test(path)) {
    return `file:///${encodeURI(path)}`;
  }
  return `file://${encodeURI(path)}`;
}

function escapeAlt(name: string): string {
  return name.replace(/[[\]]/g, "");
}

function markdownImageSources(line: string): LocalImagePath[] {
  const found: LocalImagePath[] = [];
  const matches = line.matchAll(MARKDOWN_IMAGE);
  for (const match of matches) {
    pushUnique(found, match[2] ?? "");
  }
  return found;
}

function rewriteMarkdownImages(line: string): string {
  return line.replace(MARKDOWN_IMAGE, (full, alt: string, src: string) => {
    const path = parseLocalImagePath(src);
    if (!path) return full;
    return `![${alt}](${toFileImageUrl(path)})`;
  });
}

/**
 * Turn a local path sitting in prose into a markdown image just above it.
 * Leaves fenced code alone. Does not invent images for http(s) URLs.
 */
export function hoistLocalImages(markdown: string): string {
  const lines = markdown.split("\n");
  const seen = new Set<string>();
  const out: string[] = [];
  let inFence = false;

  for (const line of lines) {
    if (FENCE.test(line)) {
      inFence = !inFence;
      out.push(line);
      continue;
    }
    if (inFence) {
      out.push(line);
      continue;
    }

    const rewritten = rewriteMarkdownImages(line);
    for (const already of markdownImageSources(rewritten)) {
      seen.add(already);
    }
    for (const path of localImagePathsIn(line)) {
      if (seen.has(path)) continue;
      seen.add(path);
      const alt = escapeAlt(localImageBasename(path));
      out.push(`![${alt}](${toFileImageUrl(path)})`);
      out.push("");
    }
    out.push(rewritten);
  }

  return out.join("\n");
}
