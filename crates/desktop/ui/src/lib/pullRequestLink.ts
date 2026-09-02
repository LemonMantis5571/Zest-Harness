/** Mark an `<a>` whose unmodified click stays in the app. */
export const INTERNAL_LINK_ATTR = "data-internal-link";

export function shouldOpenPullRequestExternally(event: {
  metaKey?: boolean;
  ctrlKey?: boolean;
  shiftKey?: boolean;
  button?: number;
}): boolean {
  if (event.button === 1) return true;
  if (event.shiftKey) return true;
  return Boolean(event.metaKey || event.ctrlKey);
}

export function pullRequestAnchorProps(url: string) {
  return {
    href: url,
    target: "_blank" as const,
    rel: "noreferrer",
    [INTERNAL_LINK_ATTR]: "",
  };
}

export function handlePullRequestClick(
  event: {
    preventDefault(): void;
    stopPropagation(): void;
    metaKey: boolean;
    ctrlKey: boolean;
    shiftKey: boolean;
    button: number;
  },
  onOpen?: () => void
): boolean {
  event.stopPropagation();
  if (!onOpen || shouldOpenPullRequestExternally(event)) {
    return false;
  }
  event.preventDefault();
  onOpen();
  return true;
}
