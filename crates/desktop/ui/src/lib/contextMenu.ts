type ContextMenuNode = {
  tagName?: unknown;
  getAttribute?: (name: string) => string | null;
  parentElement?: ContextMenuNode | null;
};

const NATIVE_EDITABLE_TAGS = new Set(["input", "select", "textarea"]);

function asContextMenuNode(target: unknown): ContextMenuNode | null {
  return target !== null && typeof target === "object"
    ? (target as ContextMenuNode)
    : null;
}

/** Keep native copy/paste menus available only in controls that accept edits. */
export function shouldPreserveNativeContextMenu(target: unknown): boolean {
  let current = asContextMenuNode(target);
  while (current) {
    const tagName =
      typeof current.tagName === "string" ? current.tagName.toLowerCase() : "";
    if (NATIVE_EDITABLE_TAGS.has(tagName)) return true;

    const contentEditable = current.getAttribute?.("contenteditable");
    if (contentEditable != null) {
      switch (contentEditable.trim().toLowerCase()) {
        case "false":
          return false;
        case "":
        case "true":
        case "plaintext-only":
          return true;
      }
    }

    current = current.parentElement ?? null;
  }
  return false;
}
