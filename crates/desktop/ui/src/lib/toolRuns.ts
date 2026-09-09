import type { ToolPart } from "./types";

/**
 * A stretch of tool calls shown as one line.
 *
 * New calls join the existing group so the list does not grow a fresh card
 * stack under a fold the user already closed. An approval still breaks out:
 * a collapsed prompt is a prompt the user cannot answer.
 */
export type ToolRun =
  | { kind: "single"; tool: ToolPart }
  | { kind: "group"; tools: ToolPart[]; summary: ToolRunSummary };

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
 * After that the group stays folded and later calls join it.
 */
export const COLLAPSE_THRESHOLD = 2;

const WRITE_TOOLS = new Set(["write_file", "edit_file"]);

function isSettled(tool: ToolPart): boolean {
  return tool.status === "done" || tool.status === "error";
}

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

/**
 * Fold tool calls into summary rows.
 *
 * A group is a contiguous run. Later calls join the group only when it is
 * still the last thing on the list — an approval in between starts a new
 * run so order stays honest. The group itself stays folded; opening it is
 * a click.
 */
export function groupToolRuns(
  tools: ToolPart[],
  threshold: number = COLLAPSE_THRESHOLD
): ToolRun[] {
  const runs: ToolRun[] = [];
  let pending: ToolPart[] = [];

  const flush = () => {
    if (pending.length === 0) return;
    if (pending.length >= threshold) {
      runs.push({
        kind: "group",
        tools: pending,
        summary: summarizeTools(pending.filter(isSettled)),
      });
    } else {
      for (const tool of pending) runs.push({ kind: "single", tool });
    }
    pending = [];
  };

  for (const tool of tools) {
    if (tool.status === "awaiting_approval") {
      flush();
      runs.push({ kind: "single", tool });
      continue;
    }
    pending.push(tool);
  }
  flush();

  return runs;
}
