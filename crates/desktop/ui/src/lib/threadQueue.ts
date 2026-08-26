import type { InputTarget, PendingInput, PreparedAttachment } from "./types.ts";

/** A user turn waiting behind the active turn for the same thread. */
export type QueuedTurn = {
  readonly id: string;
  readonly threadId: string;
  readonly target?: InputTarget;
  readonly text: string;
  readonly attachments: ReadonlyArray<PreparedAttachment>;
  readonly createdAt: number;
};

/** Project the Rust-owned queue into the existing compact composer view. */
export function pendingInputToQueuedTurn(
  input: PendingInput,
  threadId: string
): QueuedTurn {
  return {
    id: input.id,
    threadId,
    target: input.target,
    text: input.text,
    attachments: input.attachments.map((attachment, index) => ({
      id: `${input.id}-attachment-${index}`,
      name: attachment.name,
      path: "",
      kind: attachment.kind ?? "file",
      status: attachment.status,
      detail: attachment.detail,
      content: attachment.content,
      mediaType: attachment.mediaType,
      dataBase64: attachment.dataBase64,
    })),
    createdAt: input.createdAt,
  };
}

export type ThreadQueueMap = Record<string, ReadonlyArray<QueuedTurn>>;

export function enqueueThreadTurn(
  queues: ThreadQueueMap,
  threadId: string,
  turn: QueuedTurn
): ThreadQueueMap {
  return {
    ...queues,
    [threadId]: [...(queues[threadId] ?? []), turn],
  };
}

export function peekThreadTurn(
  queues: ThreadQueueMap,
  threadId: string
): QueuedTurn | undefined {
  return queues[threadId]?.[0];
}

export function removeThreadTurn(
  queues: ThreadQueueMap,
  threadId: string,
  turnId: string
): ThreadQueueMap {
  const current = queues[threadId];
  if (!current) return queues;

  const remaining = current.filter((turn) => turn.id !== turnId);
  if (remaining.length === current.length) return queues;

  const next = { ...queues };
  if (remaining.length > 0) {
    next[threadId] = remaining;
  } else {
    delete next[threadId];
  }
  return next;
}

export function updateThreadTurn(
  queues: ThreadQueueMap,
  threadId: string,
  turnId: string,
  text: string
): ThreadQueueMap {
  const current = queues[threadId];
  if (!current) return queues;

  let changed = false;
  const updated = current.map((turn) => {
    if (turn.id !== turnId) return turn;
    changed = true;
    return { ...turn, text };
  });
  return changed ? { ...queues, [threadId]: updated } : queues;
}

export function threadQueueCount(
  queues: ThreadQueueMap,
  threadId: string
): number {
  return queues[threadId]?.length ?? 0;
}

/** Only followups can start a fresh idle turn; steer/inject wait for a turn. */
export function hasResumableThreadTurn(turns: ReadonlyArray<QueuedTurn>): boolean {
  return turns.some((turn) => turn.target === "followup" || turn.target == null);
}
