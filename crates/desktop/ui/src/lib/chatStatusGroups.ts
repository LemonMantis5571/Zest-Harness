import {
  needsInput,
  type ThreadActivity,
  type ThreadActivityMap,
} from "./threadActivity.ts";
import type { ProjectChats, ThreadSummary } from "./types.ts";

/**
 * The compact list grouped by what each chat is doing rather than where it
 * lives.
 *
 * A chat waiting on you comes first, because nothing happens there until you
 * act; then the ones still running; then everything else. Sections come from
 * live `threadActivity`, so a row moves the moment its turn changes state.
 */
export type ChatStatusSection = "needs_input" | "working" | "recent";

export const CHAT_STATUS_SECTIONS: ReadonlyArray<ChatStatusSection> = [
  "needs_input",
  "working",
  "recent",
];

export type ChatStatusRow = { project: ProjectChats; thread: ThreadSummary };

export type ChatStatusGroups = Readonly<Record<ChatStatusSection, ChatStatusRow[]>>;

export function chatStatusSection(activity: ThreadActivity | undefined): ChatStatusSection {
  if (needsInput(activity)) return "needs_input";
  if (activity?.state === "working") return "working";
  return "recent";
}

/** The quiet label naming where a chat lives, shown when grouping by status. */
export function chatProjectLabel(project: ProjectChats): string {
  return project.path === null ? "No project" : project.name;
}

export function groupChatsByStatus(
  projects: ReadonlyArray<ProjectChats>,
  activity: ThreadActivityMap
): ChatStatusGroups {
  const groups: Record<ChatStatusSection, ChatStatusRow[]> = {
    needs_input: [],
    working: [],
    recent: [],
  };
  const seen = new Set<string>();
  for (const project of projects) {
    for (const thread of project.threads) {
      if (seen.has(thread.id)) continue;
      seen.add(thread.id);
      groups[chatStatusSection(activity[thread.id])].push({ project, thread });
    }
  }

  // Live sections put the newest turn first. A turn's start never changes, so
  // rows hold still while they run instead of reshuffling on every save.
  const turnStart = (row: ChatStatusRow) =>
    activity[row.thread.id]?.startedAt ?? row.thread.updatedAt * 1000;
  const newestTurnFirst = (left: ChatStatusRow, right: ChatStatusRow) =>
    turnStart(right) - turnStart(left);
  groups.needs_input.sort(newestTurnFirst);
  groups.working.sort(newestTurnFirst);

  // Across projects, the order a single project already uses: pinned, then
  // most recently updated.
  groups.recent.sort(
    (left, right) =>
      Number(right.thread.pinned) - Number(left.thread.pinned) ||
      right.thread.updatedAt - left.thread.updatedAt
  );
  return groups;
}
