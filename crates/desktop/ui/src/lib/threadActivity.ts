import type { ChatEvent } from "./types";

/**
 * What each chat is doing right now, including the ones you are not looking at.
 *
 * Chat events already arrive for every thread — they are broadcast app-wide and
 * carry a `thread_id` — but the reducer drops anything belonging to another
 * thread as stale, because it is building one transcript. That is right for the
 * transcript and wrong for the sidebar: a chat left running while you work
 * elsewhere is exactly the one you want to see the state of.
 *
 * So this consumes the same stream *before* the staleness check and keeps one
 * small record per thread. It stores no message text: this answers "is that
 * chat still going, and on what", not "what did it say".
 */

export type ThreadActivityState = "working" | "awaiting_approval" | "idle";

export type ThreadActivity = {
  state: ThreadActivityState;
  /** Tool currently running, when one is. */
  tool?: string;
  /** The most recent thing that happened, for a one-line summary. */
  lastAction?: string;
  /** Epoch ms the current turn began, so elapsed time can be shown. */
  startedAt?: number;
};

export type ThreadActivityMap = Readonly<Record<string, ThreadActivity>>;

/** A turn is over; keep the thread known but stop claiming it is busy. */
const IDLE: ThreadActivity = { state: "idle" };

function describe(event: ChatEvent): string | undefined {
  switch (event.kind) {
    case "tool_call_start":
      return event.name;
    case "tool_call_result":
      return event.isError ? `${event.name} failed` : event.name;
    case "provider_activity":
      return event.title;
    case "approval_needed":
      return `${event.tool_name} needs approval`;
    default:
      return undefined;
  }
}

/**
 * Fold one event into the activity map.
 *
 * Pure, and `now` is a parameter, so elapsed-time behaviour is testable without
 * waiting for a clock.
 */
export function reduceThreadActivity(
  map: ThreadActivityMap,
  event: ChatEvent,
  now: number
): ThreadActivityMap {
  const id = event.thread_id;
  if (!id) return map;
  const current = map[id];

  const put = (next: ThreadActivity): ThreadActivityMap =>
    // Identity is preserved when nothing changed, so React can skip the render
    // — this runs on every delta of every live chat.
    current &&
    current.state === next.state &&
    current.tool === next.tool &&
    current.lastAction === next.lastAction &&
    current.startedAt === next.startedAt
      ? map
      : { ...map, [id]: next };

  switch (event.kind) {
    case "user":
      // The user's message is what starts a turn, so it is where the clock
      // starts too — not the first token, which can be seconds later.
      return put({ state: "working", startedAt: now });

    case "assistant_start":
      return put({
        state: "working",
        startedAt: current?.startedAt ?? now,
        lastAction: current?.lastAction,
      });

    case "tool_call_start":
      return put({
        state: "working",
        startedAt: current?.startedAt ?? now,
        tool: event.name,
        lastAction: describe(event),
      });

    case "tool_call_result":
      return put({
        state: "working",
        startedAt: current?.startedAt ?? now,
        // The tool is finished; the thread is still working on the turn.
        tool: undefined,
        lastAction: describe(event),
      });

    case "provider_activity":
      return put({
        state: "working",
        startedAt: current?.startedAt ?? now,
        tool: event.status === "running" ? event.title : undefined,
        lastAction: describe(event),
      });

    case "approval_needed":
      // Distinct from working: nothing is happening until a person answers,
      // and a chat waiting on you is the one worth surfacing hardest.
      return put({
        state: "awaiting_approval",
        startedAt: current?.startedAt ?? now,
        lastAction: describe(event),
      });

    case "done":
    case "error":
    case "cancelled":
      return put(IDLE);

    default:
      return map;
  }
}

/** Threads that are doing something, for a count or a filter. */
export function activeThreadIds(map: ThreadActivityMap): string[] {
  return Object.entries(map)
    .filter(([, activity]) => activity.state !== "idle")
    .map(([id]) => id);
}

/** Human label for a tool or activity name (`web_search` → `web search`). */
export function formatActivityAction(value: string): string {
  return value.replaceAll("_", " ");
}

/**
 * What the live turn is doing right now — a running tool or provider step.
 *
 * Finished actions stay out: those belong to the transcript, not a status line.
 */
export function currentTurnAction(
  message:
    | {
        role: string;
        tools?: ReadonlyArray<{ name: string; status: string }>;
        providerActivity?: ReadonlyArray<{ title: string; status: string }>;
      }
    | undefined,
  activity?: ThreadActivity
): string | undefined {
  if (message?.role === "assistant") {
    const runningTool = message.tools
      ?.slice()
      .reverse()
      .find((tool) => tool.status === "running");
    if (runningTool) return formatActivityAction(runningTool.name);
    const runningActivity = message.providerActivity?.find(
      (item) => item.status === "running"
    );
    if (runningActivity?.title.trim()) return runningActivity.title;
  }
  if (activity?.tool) return formatActivityAction(activity.tool);
  return undefined;
}

/**
 * Compact elapsed label: `8s`, `6m 51s`, `1h 04m`.
 *
 * Seconds are dropped past an hour — at that point they are noise, and the
 * label has to stay narrow enough for a sidebar row.
 */
export function elapsedLabel(startedAt: number | undefined, now: number): string | undefined {
  if (!startedAt || now < startedAt) return undefined;
  const seconds = Math.floor((now - startedAt) / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${String(seconds % 60).padStart(2, "0")}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${String(minutes % 60).padStart(2, "0")}m`;
}
