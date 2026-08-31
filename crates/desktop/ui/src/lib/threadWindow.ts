import type { ChatMessage } from "./types.ts";

/** Keep in lockstep with `THREAD_WINDOW_USER_TURNS` in zest-core. */
export const THREAD_WINDOW_USER_TURNS = 10;
/** Keep in lockstep with `THREAD_OLDER_USER_TURNS` in zest-core. */
export const THREAD_OLDER_USER_TURNS = 20;

export type MessageWindow = {
  messages: ChatMessage[];
  hasOlder: boolean;
  hasNewer: boolean;
  hiddenUserTurns: number;
};

function userStartIndices(messages: ChatMessage[]): number[] {
  return messages.flatMap((message, index) =>
    message.role === "user" ? [index] : []
  );
}

function fromUserTurns(
  messages: ChatMessage[],
  starts: number[],
  startTurn: number,
  userTurns: number
): MessageWindow {
  const limit = Math.max(1, userTurns);
  if (starts.length === 0) {
    return {
      messages: messages.slice(),
      hasOlder: false,
      hasNewer: false,
      hiddenUserTurns: 0,
    };
  }
  const start = Math.min(startTurn, Math.max(0, starts.length - 1));
  const endTurn = Math.min(start + limit, starts.length);
  const msgStart = start === 0 ? 0 : (starts[start] ?? 0);
  const msgEnd =
    endTurn === starts.length ? messages.length : (starts[endTurn] ?? messages.length);
  return {
    messages: messages.slice(msgStart, msgEnd),
    hasOlder: start > 0,
    hasNewer: endTurn < starts.length,
    hiddenUserTurns: start,
  };
}

/** Last `userTurns` user messages and everything after the first of those. */
export function tailUserTurns(
  messages: ChatMessage[],
  userTurns: number
): MessageWindow {
  const limit = Math.max(1, userTurns);
  const starts = userStartIndices(messages);
  return fromUserTurns(
    messages,
    starts,
    Math.max(0, starts.length - limit),
    limit
  );
}

/** A window of `userTurns` that contains `focusId`. Prefers the tail. */
export function aroundUserTurns(
  messages: ChatMessage[],
  focusId: string,
  userTurns: number
): MessageWindow {
  const limit = Math.max(1, userTurns);
  const starts = userStartIndices(messages);
  const focus = messages.findIndex((message) => message.id === focusId);
  if (focus < 0) {
    throw new Error("that message is not in this chat");
  }
  let focusTurn = 0;
  for (let index = starts.length - 1; index >= 0; index -= 1) {
    const start = starts[index];
    if (start !== undefined && start <= focus) {
      focusTurn = index;
      break;
    }
  }
  const startTurn = Math.min(focusTurn, Math.max(0, starts.length - limit));
  return fromUserTurns(messages, starts, startTurn, limit);
}

/** The page of user turns that ends just before `beforeId`. */
export function olderUserTurns(
  messages: ChatMessage[],
  beforeId: string,
  userTurns: number
): MessageWindow {
  const end = messages.findIndex((message) => message.id === beforeId);
  if (end < 0) {
    throw new Error("that message is not in this chat");
  }
  return tailUserTurns(messages.slice(0, end), userTurns);
}

/** The page of user turns that starts just after `afterId`. */
export function newerUserTurns(
  messages: ChatMessage[],
  afterId: string,
  userTurns: number
): MessageWindow {
  const pos = messages.findIndex((message) => message.id === afterId);
  if (pos < 0) {
    throw new Error("that message is not in this chat");
  }
  const rest = messages.slice(pos + 1);
  const skip = rest.findIndex((message) => message.role === "user");
  const start = skip < 0 ? messages.length : pos + 1 + skip;
  const hiddenUserTurns = userStartIndices(messages.slice(0, start)).length;
  const page = fromUserTurns(
    messages.slice(start),
    userStartIndices(messages.slice(start)),
    0,
    userTurns
  );
  return {
    ...page,
    hasOlder: start > 0,
    hiddenUserTurns,
  };
}

/** Prepend an older page. Duplicate ids stay with the already-loaded row. */
export function prependOlderMessages(
  current: ChatMessage[],
  older: ChatMessage[]
): ChatMessage[] {
  if (older.length === 0) return current;
  const seen = new Set(current.map((message) => message.id));
  const incoming = older.filter((message) => !seen.has(message.id));
  return incoming.length === 0 ? current : [...incoming, ...current];
}

/** Append a newer page. Duplicate ids stay with the already-loaded row. */
export function appendNewerMessages(
  current: ChatMessage[],
  newer: ChatMessage[]
): ChatMessage[] {
  if (newer.length === 0) return current;
  const seen = new Set(current.map((message) => message.id));
  const incoming = newer.filter((message) => !seen.has(message.id));
  return incoming.length === 0 ? current : [...current, ...incoming];
}

export function firstLoadedMessageId(messages: ChatMessage[]): string | null {
  return messages[0]?.id ?? null;
}

export function lastLoadedMessageId(messages: ChatMessage[]): string | null {
  return messages.at(-1)?.id ?? null;
}
