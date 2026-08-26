export type ReadingDiff = {
  diff: string;
  hiddenImports: number;
  foldedContextLines: number;
};

const IMPORT_START = /^(?:import\b|from\s+\S+\s+import\b|use\s+[^;]+;|#include\b|require\s*\()/;

function isDiffMetadata(line: string): boolean {
  return (
    line.startsWith("diff ") ||
    line.startsWith("index ") ||
    line.startsWith("--- ") ||
    line.startsWith("+++ ") ||
    line.startsWith("@@") ||
    line.startsWith("rename ") ||
    line.startsWith("copy ")
  );
}

function isImportLine(line: string): boolean {
  if (line.length < 2 || isDiffMetadata(line)) return false;
  const marker = line[0];
  if (marker !== "+" && marker !== "-" && marker !== " ") return false;
  return IMPORT_START.test(line.slice(1).trim());
}

function isContextLine(line: string): boolean {
  return line.startsWith(" ") && !isDiffMetadata(line);
}

/**
 * Build a conservative, display-only reading diff.
 *
 * This intentionally does not ask a model or produce an applicable patch.
 * The exact diff remains the source of truth; this view only hides obvious
 * import churn and folds long unchanged context runs.
 */
export function makeReadingDiff(raw: string): ReadingDiff {
  const source = raw.split("\n");
  const output: string[] = [];
  let hiddenImports = 0;
  let foldedContextLines = 0;
  let contextRun: string[] = [];

  const flushContext = () => {
    if (contextRun.length > 4) {
      output.push(contextRun[0], " …", contextRun[contextRun.length - 1]);
      foldedContextLines += contextRun.length - 2;
    } else {
      output.push(...contextRun);
    }
    contextRun = [];
  };

  for (const line of source) {
    if (isImportLine(line)) {
      flushContext();
      hiddenImports += 1;
      continue;
    }
    if (isContextLine(line)) {
      contextRun.push(line);
      continue;
    }
    flushContext();
    output.push(line);
  }
  flushContext();

  return {
    diff: output.join("\n"),
    hiddenImports,
    foldedContextLines,
  };
}
