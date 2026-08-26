/** Sticky composer draft per thread (localStorage). */

const PREFIX = "zest:draft:";

export function loadDraft(threadId: string | null | undefined): string {
  if (!threadId || typeof localStorage === "undefined") return "";
  try {
    return localStorage.getItem(PREFIX + threadId) ?? "";
  } catch {
    return "";
  }
}

export function saveDraft(threadId: string | null | undefined, draft: string) {
  if (!threadId || typeof localStorage === "undefined") return;
  try {
    const key = PREFIX + threadId;
    if (draft.trim()) {
      localStorage.setItem(key, draft);
    } else {
      localStorage.removeItem(key);
    }
  } catch {
    /* ignore quota / private mode */
  }
}
