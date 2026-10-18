import type { ToolPart } from "./types";

/** What a folded run of tool calls did, for its Chain of Thought line. */
export type ToolRunSummary = {
  /** `bash` invocations. */
  commands: number;
  /** Distinct paths touched by write_file / edit_file. */
  filesEdited: number;
  /** Reads, searches, listings — everything that changed nothing. */
  inspections: number;
  added: number;
  removed: number;
  errors: number;
  label: string;
};

/**
 * Fold once there is more than one row.
 *
 * One tool stays a row: "Ran 1 lookup" is longer than the path it would hide.
 * After that a contiguous run becomes one Chain of Thought line.
 */
export const COLLAPSE_THRESHOLD = 2;

const WRITE_TOOLS = new Set(["write_file", "edit_file"]);

/**
 * Count `+`/`-` lines in a unified diff.
 *
 * `---`/`+++` file headers start with the same characters as content lines, so
 * they are skipped explicitly; counting them would add two phantom lines to
 * every single-file edit.
 */
export function countDiffLines(diff: string | undefined): {
  added: number;
  removed: number;
} {
  if (!diff) return { added: 0, removed: 0 };
  let added = 0;
  let removed = 0;
  for (const line of diff.split("\n")) {
    if (line.startsWith("+++") || line.startsWith("---")) continue;
    if (line.startsWith("+")) added += 1;
    else if (line.startsWith("-")) removed += 1;
  }
  return { added, removed };
}

export function summarizeTools(tools: ToolPart[]): ToolRunSummary {
  let commands = 0;
  let inspections = 0;
  let added = 0;
  let removed = 0;
  let errors = 0;
  const editedPaths = new Set<string>();

  for (const tool of tools) {
    if (tool.status === "error") errors += 1;
    if (tool.name === "bash") {
      commands += 1;
    } else if (WRITE_TOOLS.has(tool.name)) {
      // Keyed by path: three edits to one file is one file changed.
      editedPaths.add(tool.path || tool.id);
      const counted = countDiffLines(tool.diff);
      added += counted.added;
      removed += counted.removed;
    } else {
      inspections += 1;
    }
  }

  const filesEdited = editedPaths.size;
  const parts: string[] = [];
  if (commands > 0) {
    parts.push(`${commands} command${commands === 1 ? "" : "s"}`);
  }
  if (filesEdited > 0) {
    parts.push(`edited ${filesEdited} file${filesEdited === 1 ? "" : "s"}`);
  }
  if (inspections > 0) {
    parts.push(`${inspections} lookup${inspections === 1 ? "" : "s"}`);
  }

  const label = parts.length > 0 ? `Ran ${parts.join(", ")}` : "Ran tools";

  return { commands, filesEdited, inspections, added, removed, errors, label };
}
