export type DiffSection = {
  path: string;
  diff: string;
  added: number;
  removed: number;
};

function unquotePath(value: string): string {
  const trimmed = value.trim().split("\t", 1)[0] ?? "";
  if (trimmed === "/dev/null") return "";
  if (trimmed.startsWith("a/") || trimmed.startsWith("b/")) {
    return trimmed.slice(2);
  }
  return trimmed;
}

function pathFromGitHeader(line: string): string {
  const payload = line.slice("diff --git ".length);
  const boundary = payload.lastIndexOf(" b/");
  if (boundary < 0) return "";
  return unquotePath(payload.slice(boundary + 1));
}

function pathFromUnifiedHeader(lines: string[]): string {
  const marker = lines.find((line) => line.startsWith("+++ "));
  return marker ? unquotePath(marker.slice(4)) : "";
}

function lineCounts(diff: string): Pick<DiffSection, "added" | "removed"> {
  let added = 0;
  let removed = 0;
  for (const line of diff.split("\n")) {
    if (line.startsWith("+") && !line.startsWith("+++")) added += 1;
    if (line.startsWith("-") && !line.startsWith("---")) removed += 1;
  }
  return { added, removed };
}

function makeSection(lines: string[], fallbackPath: string): DiffSection {
  const diff = lines.join("\n");
  const path = lines[0]?.startsWith("diff --git ")
    ? pathFromGitHeader(lines[0])
    : pathFromUnifiedHeader(lines);
  return {
    path: path || fallbackPath || "Changed file",
    diff,
    ...lineCounts(diff),
  };
}

/** Split a unified diff into the file sections shown by the review sidebar. */
export function splitDiffSections(raw: string, fallbackPath = ""): DiffSection[] {
  const lines = raw.split("\n");
  const gitStarts = lines.reduce<number[]>((result, line, index) => {
    if (line.startsWith("diff --git ")) result.push(index);
    return result;
  }, []);

  if (gitStarts.length > 0) {
    return gitStarts.map((start, index) =>
      makeSection(lines.slice(start, gitStarts[index + 1] ?? lines.length), fallbackPath)
    );
  }

  const unifiedStarts = lines.reduce<number[]>((result, line, index) => {
    if (line.startsWith("--- ") && lines[index + 1]?.startsWith("+++ ")) {
      result.push(index);
    }
    return result;
  }, []);

  if (unifiedStarts.length > 1) {
    return unifiedStarts.map((start, index) =>
      makeSection(lines.slice(start, unifiedStarts[index + 1] ?? lines.length), fallbackPath)
    );
  }

  return [makeSection(lines, fallbackPath)];
}
