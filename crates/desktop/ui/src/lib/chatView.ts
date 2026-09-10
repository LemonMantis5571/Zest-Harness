export type ChatViewMode = "project" | "compact";

export const DEFAULT_CHAT_VIEW_MODE: ChatViewMode = "project";
export const CHAT_VIEW_STORAGE_KEY = "zest.chatViewMode";
export const CHAT_VIEW_CHANGED_EVENT = "zest:chat-view-changed";

let inMemoryChatViewMode: ChatViewMode | null = null;

function isChatViewMode(value: string | null): value is ChatViewMode {
  return value === "project" || value === "compact";
}

export function getSavedChatViewMode(): ChatViewMode {
  if (typeof localStorage === "undefined") {
    return inMemoryChatViewMode ?? DEFAULT_CHAT_VIEW_MODE;
  }
  try {
    const value = localStorage.getItem(CHAT_VIEW_STORAGE_KEY);
    return isChatViewMode(value) ? value : DEFAULT_CHAT_VIEW_MODE;
  } catch {
    return inMemoryChatViewMode ?? DEFAULT_CHAT_VIEW_MODE;
  }
}

export function applyChatViewMode(mode: ChatViewMode): ChatViewMode {
  inMemoryChatViewMode = mode;
  try {
    localStorage.setItem(CHAT_VIEW_STORAGE_KEY, mode);
  } catch {
    /* The setting still applies for this session if storage is unavailable. */
  }
  if (typeof window !== "undefined") {
    window.dispatchEvent(new Event(CHAT_VIEW_CHANGED_EVENT));
  }
  return mode;
}

export function subscribeChatViewChange(listener: () => void) {
  if (typeof window === "undefined") return () => {};
  window.addEventListener(CHAT_VIEW_CHANGED_EVENT, listener);
  return () => window.removeEventListener(CHAT_VIEW_CHANGED_EVENT, listener);
}
