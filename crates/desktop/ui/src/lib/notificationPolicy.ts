/** A turn must occupy the user's attention long enough to merit a completion notice. */
export const LONG_TURN_NOTIFICATION_MS = 10_000;

export function isLongTurn(durationMs: number) {
  return durationMs >= LONG_TURN_NOTIFICATION_MS;
}

/**
 * Events that may ask for the user's attention.
 *
 * Delegation events belong to their parent chat, but their state lives in the
 * workbench panel rather than the transcript, so that chat being on screen
 * does not mean the outcome is.
 */
export const ATTENTION_KINDS = [
  "approval_needed",
  "input_needed",
  "response_ready",
  "request_failed",
  "job_finished",
  "job_failed",
  "delegation_ready",
  "delegation_changes_requested",
  "delegation_blocked",
  "delegation_failed",
  "delegation_applied",
  "delegation_cancelled",
] as const;

export type AttentionKind = (typeof ATTENTION_KINDS)[number];

export type AttentionRoute = "none" | "toast" | "os";

export type AttentionContext = {
  /** The event's chat is visible: open in single view, or a pane of the active split group. */
  onScreen: boolean;
  /** Zest is the focused, unminimized window. */
  focused: boolean;
  /** The user's own click in this window caused the event, e.g. cancelling a delegation. */
  byUser?: boolean;
};

/** Events whose card, answer, or error is drawn in their chat's transcript. */
const SHOWN_IN_TRANSCRIPT: ReadonlySet<AttentionKind> = new Set([
  "approval_needed",
  "input_needed",
  "response_ready",
  "request_failed",
]);

/**
 * The agent already hears about every finished background job through a
 * notice injected into its chat, so the user only needs to hear about a
 * success when they are away.
 */
const AWAY_ONLY: ReadonlySet<AttentionKind> = new Set(["job_finished"]);

/**
 * Where an attention event goes, if anywhere.
 *
 * Live state belongs on screen; a ping is for something that needs the user
 * while they are not looking. An event already drawn in the visible
 * transcript says nothing while Zest is focused, and anything the user is
 * away for becomes an OS notification instead of a toast they cannot see.
 * The user's own click needs no announcement; its result is what they see.
 */
export function attentionRoute(
  kind: AttentionKind,
  { onScreen, focused, byUser = false }: AttentionContext
): AttentionRoute {
  if (byUser) return "none";
  if (!focused) return "os";
  if (AWAY_ONLY.has(kind)) return "none";
  if (onScreen && SHOWN_IN_TRANSCRIPT.has(kind)) return "none";
  return "toast";
}

const CHAT_NAME_MAX_CHARS = 48;
const EXCERPT_MAX_CHARS = 140;

function clip(text: string, max: number) {
  return text.length > max ? `${text.slice(0, max - 1).trimEnd()}…` : text;
}

/**
 * How an alert names its chat: the sidebar title, else the first prompt (a
 * chat is titled after its first turn), else what the sidebar shows.
 */
export function attentionChatName(title: string | undefined, firstPrompt: string | undefined) {
  const name =
    title?.trim() || firstPrompt?.replace(/\s+/g, " ").trim() || "Untitled chat";
  return clip(name, CHAT_NAME_MAX_CHARS);
}

/** "Approval needed · Fix the login redirect" */
export function attentionTitle(event: string, chatName: string) {
  return `${event} · ${chatName}`;
}

/**
 * The start of a reply as plain text, for a notification body. OS
 * notifications show Markdown syntax literally, so the common marks go.
 */
export function replyExcerpt(text: string) {
  const plain = text
    .replace(/```[\s\S]*?(?:```|$)/g, " ")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/^\s{0,3}(?:#{1,6}|>|[-*+])\s+/gm, "")
    .replace(/\*\*|__|`/g, "")
    .replace(/\s+/g, " ")
    .trim();
  return clip(plain, EXCERPT_MAX_CHARS);
}

/**
 * Group key for stacked toasts.
 *
 * Errors keep the description so distinct failures stay separate. Success and
 * info group on title: extra copy such as "type to start a new chat" must not
 * fork a second "Chat deleted" card behind the count badge.
 */
export function notificationFingerprint(
  type: string | undefined,
  title: string,
  description: string
) {
  if (type === "success" || type === "info") {
    return JSON.stringify([type, title]);
  }
  return JSON.stringify([type ?? "", title, description]);
}

/** Prefer a non-empty hint when collapsing two success toasts into one card. */
export function mergeToastDescription(existing: string, incoming: string) {
  return incoming || existing;
}

