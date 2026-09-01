import { ignoreExpectedFailure } from "./backgroundFailure.ts";

/** Match http(s) URLs. Reject javascript:, file:, and anything else. */
export function safeHttpUrl(value: string | null | undefined): string | null {
  const href = value?.trim();
  if (!href) return null;
  try {
    const url = new URL(href);
    return url.protocol === "http:" || url.protocol === "https:" ? url.toString() : null;
  } catch {
    return null;
  }
}

type ClickNode = {
  tagName?: unknown;
  href?: unknown;
  getAttribute?: (name: string) => string | null;
  hasAttribute?: (name: string) => boolean;
  parentElement?: ClickNode | null;
};

function asClickNode(target: unknown): ClickNode | null {
  return target !== null && typeof target === "object" ? (target as ClickNode) : null;
}

function closestAnchor(target: unknown): ClickNode | null {
  let current = asClickNode(target);
  while (current) {
    const tagName =
      typeof current.tagName === "string" ? current.tagName.toLowerCase() : "";
    if (tagName === "a") return current;
    current = current.parentElement ?? null;
  }
  return null;
}

function hrefFromAnchor(anchor: ClickNode): string | null {
  const attr = anchor.getAttribute?.("href");
  if (attr) return safeHttpUrl(attr);
  return typeof anchor.href === "string" ? safeHttpUrl(anchor.href) : null;
}

/**
 * Decide whether a click on an `<a>` should leave the webview.
 * Alt-click is left for the OS "save as" gesture.
 */
export function externalHttpUrlFromClick(event: {
  defaultPrevented: boolean;
  button: number;
  altKey?: boolean;
  target: unknown;
}): string | null {
  if (event.defaultPrevented) return null;
  if (event.button !== 0 && event.button !== 1) return null;
  if (event.altKey) return null;
  const anchor = closestAnchor(event.target);
  if (!anchor) return null;
  if (anchor.hasAttribute?.("download")) return null;
  return hrefFromAnchor(anchor);
}

export async function openExternalUrl(href: string): Promise<void> {
  const url = safeHttpUrl(href);
  if (!url) return;
  try {
    const chrome = await import("./windowChrome.ts");
    await chrome.openExternalUrl(url);
  } catch {
    if (typeof window !== "undefined" && typeof window.open === "function") {
      window.open(url, "_blank", "noopener,noreferrer");
    }
  }
}

function onMouseEvent(event: MouseEvent) {
  const url = externalHttpUrlFromClick(event);
  if (!url) return;
  event.preventDefault();
  void openExternalUrl(url).catch((error) =>
    ignoreExpectedFailure(error, "open external url")
  );
}

/** Capture-phase so Tauri never tries to navigate the webview. */
export function installExternalLinkHandling(): void {
  document.addEventListener("click", onMouseEvent, true);
  document.addEventListener("auxclick", onMouseEvent, true);
}
