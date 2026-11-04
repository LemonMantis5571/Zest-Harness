import { needsInput, type ThreadActivity } from "./threadActivity.ts";

/**
 * What a pane's strip says about its chat while the transcript is put away.
 *
 * The same facts as the sidebar rows, in the same order of urgency: a chat
 * waiting on you, then one still working, then one that finished while nobody
 * was looking. Nothing here is stored; it is read from `threadActivity` and the
 * unread set each time the strip renders.
 */
export type PaneStatusKind = "needs_input" | "working" | "unread" | "idle" | "empty";

export type PaneStatus = {
  kind: PaneStatusKind;
  /** Plain words for the strip and for its accessible name. */
  label: string;
};

export function splitPaneStatus({
  hasChat,
  activity,
  sending = false,
  unread = false,
}: {
  hasChat: boolean;
  activity?: ThreadActivity;
  /** The pane's own streaming flag, for a turn the activity map has not seen yet. */
  sending?: boolean;
  unread?: boolean;
}): PaneStatus {
  if (!hasChat) return { kind: "empty", label: "Empty" };
  if (needsInput(activity)) {
    return {
      kind: "needs_input",
      label: activity?.state === "awaiting_input" ? "Waiting for your answer" : "Awaiting approval",
    };
  }
  if (activity?.state === "working" || sending) return { kind: "working", label: "Working" };
  if (unread) return { kind: "unread", label: "Unread" };
  return { kind: "idle", label: "Idle" };
}

/** The strip's accessible name: the action it performs, then what the chat is doing. */
export function paneStripLabel(title: string, status: PaneStatus): string {
  return `Restore ${title}, ${status.label.toLowerCase()}`;
}
