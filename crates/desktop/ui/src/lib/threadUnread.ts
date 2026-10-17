import { parseJson } from "./json.ts";
import type { ChatEvent } from "./types.ts";

/**
 * Chats whose turn ended while you were looking at another one.
 *
 * The sidebar shows them in bold until you open them, the way a mail client
 * shows unread mail. `ThreadSummary` cannot say this: `updatedAt` moves on
 * every saved message, and nothing in the store knows which chat was on
 * screen when a turn ended. The UI knows that at the moment the turn ends, so
 * the decision is made here and the set is kept in localStorage, which
 * survives a restart on this machine. A turn that ends while the app is closed
 * is not marked.
 */
export type UnreadThreads = ReadonlySet<string>;

export const UNREAD_THREADS_STORAGE_KEY = "zest.unreadThreads";
export const UNREAD_THREADS_CHANGED_EVENT = "zest:unread-threads-changed";

/** Deleted chats are never opened again, so the oldest marks are dropped. */
export const MAX_UNREAD_THREADS = 200;

const EMPTY: UnreadThreads = new Set();

/**
 * Fold one chat event into the set.
 *
 * Only a turn that ended on its own counts. A cancel is something you did, so
 * there is nothing new to see.
 */
export function reduceUnreadThreads(
  unread: UnreadThreads,
  event: ChatEvent,
  viewingThreadId: string | null
): UnreadThreads {
  const id = event.thread_id;
  if (!id || id === viewingThreadId || unread.has(id)) return unread;
  if (event.kind !== "done" && event.kind !== "error") return unread;
  const next = new Set(unread);
  next.add(id);
  // A Set iterates in insertion order, so the first entry is the stalest.
  for (const stale of next) {
    if (next.size <= MAX_UNREAD_THREADS) break;
    next.delete(stale);
  }
  return next;
}

/** The chat is on screen now, so whatever it finished has been seen. */
export function withThreadSeen(unread: UnreadThreads, threadId: string): UnreadThreads {
  if (!unread.has(threadId)) return unread;
  const next = new Set(unread);
  next.delete(threadId);
  return next;
}

function parseUnreadThreads(raw: string | null): UnreadThreads {
  if (!raw) return EMPTY;
  const parsed = parseJson(raw);
  if (!Array.isArray(parsed)) return EMPTY;
  return new Set(
    parsed
      .filter((id): id is string => typeof id === "string" && id.length > 0)
      .slice(-MAX_UNREAD_THREADS)
  );
}

let memoryRaw: string | null = null;
let cachedRaw: string | null | undefined;
let cachedSet: UnreadThreads = EMPTY;

function readRaw(): string | null {
  if (typeof localStorage === "undefined") return memoryRaw;
  try {
    return localStorage.getItem(UNREAD_THREADS_STORAGE_KEY);
  } catch {
    return memoryRaw;
  }
}

/** The current set. The same object until it changes, for `useSyncExternalStore`. */
export function getUnreadThreads(): UnreadThreads {
  const raw = readRaw();
  if (raw !== cachedRaw) {
    cachedRaw = raw;
    cachedSet = parseUnreadThreads(raw);
  }
  return cachedSet;
}

function commit(next: UnreadThreads) {
  if (next === getUnreadThreads()) return;
  const raw = JSON.stringify([...next]);
  memoryRaw = raw;
  try {
    localStorage.setItem(UNREAD_THREADS_STORAGE_KEY, raw);
  } catch {
    /* Still applies for this session if storage is unavailable. */
  }
  cachedRaw = raw;
  cachedSet = next;
  if (typeof window !== "undefined") {
    window.dispatchEvent(new Event(UNREAD_THREADS_CHANGED_EVENT));
  }
}

export function recordUnreadEvent(event: ChatEvent, viewingThreadId: string | null) {
  commit(reduceUnreadThreads(getUnreadThreads(), event, viewingThreadId));
}

export function markThreadSeen(threadId: string) {
  commit(withThreadSeen(getUnreadThreads(), threadId));
}

export function subscribeUnreadThreads(listener: () => void) {
  if (typeof window === "undefined") return () => {};
  const onStorage = (event: StorageEvent) => {
    if (event.key === UNREAD_THREADS_STORAGE_KEY) listener();
  };
  window.addEventListener(UNREAD_THREADS_CHANGED_EVENT, listener);
  window.addEventListener("storage", onStorage);
  return () => {
    window.removeEventListener(UNREAD_THREADS_CHANGED_EVENT, listener);
    window.removeEventListener("storage", onStorage);
  };
}
