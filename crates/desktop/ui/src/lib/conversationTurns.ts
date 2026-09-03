import type { ChatMessage, ThreadCheckpoint } from "./types.ts";

export type ConversationTurnStatus = "pending" | "working" | "done" | "error";

export type ConversationTurn = {
  id: string;
  messageId: string;
  number: number;
  preview: string;
  toolCount: number;
  status: ConversationTurnStatus;
  checkpoint?: ThreadCheckpoint;
};

const MAX_PREVIEW_CHARS = 180;

function compactPreview(message: Extract<ChatMessage, { role: "user" }>): string {
  const text = message.text.replace(/\s+/g, " ").trim();
  if (text) return text.slice(0, MAX_PREVIEW_CHARS);
  const attachmentCount = message.attachments?.length ?? 0;
  return attachmentCount > 0
    ? `${attachmentCount} attachment${attachmentCount === 1 ? "" : "s"}`
    : "Empty prompt";
}

export type ConversationTurnOptions = {
  /** User turns not present in `messages`. Rail numbers start after this. */
  turnNumberOffset?: number;
  /** `messages` is a tail window; `messageCount` is not an index into it. */
  windowed?: boolean;
};

function checkpointAnchor(
  checkpoint: ThreadCheckpoint,
  messages: ChatMessage[],
  windowed: boolean
): string | undefined {
  if (checkpoint.anchorMessageId) {
    if (messages.some((message) => message.id === checkpoint.anchorMessageId)) {
      return checkpoint.anchorMessageId;
    }
    if (windowed) return undefined;
  }

  if (windowed) return undefined;

  const candidates = [checkpoint.messageCount, checkpoint.messageCount - 1]
    .filter((index) => index >= 0 && index < messages.length)
    .map((index) => messages[index])
    .filter((message): message is Extract<ChatMessage, { role: "user" }> => message?.role === "user");
  if (candidates[0]) return candidates[0].id;

  const end = Math.min(messages.length, Math.max(0, checkpoint.messageCount));
  return [...messages.slice(0, end)].reverse().find((message) => message.role === "user")?.id;
}

function turnStatus(assistantMessages: Extract<ChatMessage, { role: "assistant" }>[]): ConversationTurnStatus {
  if (assistantMessages.some((message) => message.error)) return "error";
  if (
    assistantMessages.some(
      (message) =>
        message.streaming ||
        message.providerActivity?.some((activity) => activity.status === "running")
    )
  ) {
    return "working";
  }
  return assistantMessages.length > 0 ? "done" : "pending";
}

/** Build the safe, navigation-only index shown by the chat turn history UI. */
export function buildConversationTurns(
  messages: ChatMessage[],
  checkpoints: ThreadCheckpoint[] = [],
  options: ConversationTurnOptions = {}
): ConversationTurn[] {
  const turnNumberOffset = options.turnNumberOffset ?? 0;
  const windowed = options.windowed === true;
  const messageIds = new Set(messages.map((m) => m.id));
  const checkpointsByMessageId = new Map<string, ThreadCheckpoint>();
  for (const checkpoint of checkpoints) {
    if (checkpoint.anchorMessageId && messageIds.has(checkpoint.anchorMessageId)) {
      checkpointsByMessageId.set(checkpoint.anchorMessageId, checkpoint);
      continue;
    }
    const anchor = checkpointAnchor(checkpoint, messages, windowed);
    if (anchor) checkpointsByMessageId.set(anchor, checkpoint);
  }

  const turns: ConversationTurn[] = [];
  let currentUserTurn: Extract<ChatMessage, { role: "user" }> | null = null;
  let currentAssistants: Extract<ChatMessage, { role: "assistant" }>[] = [];

  const flushTurn = () => {
    if (!currentUserTurn) return;
    turns.push({
      id: `turn-${currentUserTurn.id}`,
      messageId: currentUserTurn.id,
      number: turnNumberOffset + turns.length + 1,
      preview: compactPreview(currentUserTurn),
      toolCount: currentAssistants.reduce((total, assistant) => total + assistant.tools.length, 0),
      status: turnStatus(currentAssistants),
      checkpoint: checkpointsByMessageId.get(currentUserTurn.id),
    });
    currentAssistants = [];
  };

  for (let index = 0; index < messages.length; index += 1) {
    const message = messages[index];
    if (message.role === "user") {
      flushTurn();
      currentUserTurn = message;
    } else if (message.role === "assistant" && currentUserTurn) {
      currentAssistants.push(message);
    }
  }
  flushTurn();
  return turns;
}
