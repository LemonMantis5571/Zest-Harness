import type { ThreadActivity } from "./threadActivity.ts";

/**
 * The one mark at the start of a compact chat row.
 *
 * It says what is going on with the chat, not who owns it: every row used to
 * lead with its provider's logo, which was the same on almost every row and so
 * said nothing. Most urgent first, because the row only has room for one.
 */
export type CompactChatMark =
  | "awaiting_approval"
  | "working"
  | "pull_request"
  | "branch"
  | "idle";

export function compactChatMark({
  activity,
  hasPullRequest,
  branchChanged,
}: {
  activity?: ThreadActivity;
  hasPullRequest: boolean;
  branchChanged: boolean;
}): CompactChatMark {
  if (activity?.state === "awaiting_approval") return "awaiting_approval";
  if (activity?.state === "working") return "working";
  if (hasPullRequest) return "pull_request";
  if (branchChanged) return "branch";
  return "idle";
}

/**
 * The provider to show beside a chat, or null when it is the one in use.
 *
 * Only the exception earns a mark: a chat on another provider opens there (or
 * as a copy), which is worth knowing before clicking it.
 */
export function foreignProvider(
  threadProviderId: string | undefined | null,
  activeProviderId: string | undefined | null
): string | null {
  if (!threadProviderId || threadProviderId === activeProviderId) return null;
  return threadProviderId;
}
