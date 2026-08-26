import type { ToolPart } from "./types";

/**
 * A stretch of finished tool calls that can be shown as one line.
 *
 * Anything still live — running, or waiting on approval — is never folded in.
 * A collapsed approval card would be a prompt the user cannot see, and a
 * collapsed spinner reads as a stall.
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
 * Below this a group costs more attention than the rows it replaces, *while the
 * turn is still working*.
 *
 * Kept high on purpose during a turn: rows folding away underneath you as each
 * call lands makes it hard to follow what the model is doing.
 */
export const COLLAPSE_THRESHOLD = 5;

/**
 * The threshold once every tool in the message has finished.
 *
 * A finished turn is something you scroll past, not something you watch, so the
 * bar for folding drops to "more than one row". Two is the floor rather than
 * one because "Ran 1 lookup" is longer than the row it would replace and hides
 * which file was read.
 */
export const SETTLED_COLLAPSE_THRESHOLD = 2;

/**
 * The threshold to group this message's tools at.
 *
 * Keyed on whether anything is still live rather than on the turn's sending
 * flag: tools routinely finish while the assistant is still writing its reply,
 * and by then the tool list is done and can be folded.
 */
export function collapseThresholdFor(tools: ToolPart[]): number {
  return tools.every(isSettled) ? SETTLED_COLLAPSE_THRESHOLD : COLLAPSE_THRESHOLD;
}

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
  if (inspections > 0 && parts.length === 0) {
    parts.push(`${inspections} lookup${inspections === 1 ? "" : "s"}`);
  }

  const label = parts.length > 0 ? `Ran ${parts.join(", ")}` : "Ran tools";

  return { commands, filesEdited, inspections, added, removed, errors, label };
}

/**
 * Fold long stretches of finished tool calls into summary rows.
 *
 * Order is preserved exactly — a group only ever replaces a contiguous run, so
 * expanding one puts the rows back where they were.
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
        summary: summarizeTools(pending),
      });
    } else {
      for (const tool of pending) runs.push({ kind: "single", tool });
    }
    pending = [];
  };

  for (const tool of tools) {
    if (isSettled(tool)) {
      pending.push(tool);
      continue;
    }
    // A live row breaks the run so it stays visible on its own.
    flush();
    runs.push({ kind: "single", tool });
  }
  flush();

  return runs;
}
