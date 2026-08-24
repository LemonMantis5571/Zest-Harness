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

function checkpointAnchor(checkpoint: ThreadCheckpoint, messages: ChatMessage[]): string | undefined {
  if (checkpoint.anchorMessageId) return checkpoint.anchorMessageId;

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
  checkpoints: ThreadCheckpoint[] = []
): ConversationTurn[] {
  const checkpointsByMessageId = new Map<string, ThreadCheckpoint>();
  for (const checkpoint of checkpoints) {
    const anchor = checkpointAnchor(checkpoint, messages);
    if (anchor) checkpointsByMessageId.set(anchor, checkpoint);
  }

  const turns: ConversationTurn[] = [];
  for (let index = 0; index < messages.length; index += 1) {
    const message = messages[index];
    if (message.role !== "user") continue;

    const nextUserIndex = messages.findIndex(
      (candidate, candidateIndex) => candidateIndex > index && candidate.role === "user"
    );
    const end = nextUserIndex < 0 ? messages.length : nextUserIndex;
    const assistantMessages = messages
      .slice(index + 1, end)
      .filter(
        (candidate): candidate is Extract<ChatMessage, { role: "assistant" }> =>
          candidate.role === "assistant"
      );

    turns.push({
      id: `turn-${message.id}`,
      messageId: message.id,
      number: turns.length + 1,
      preview: compactPreview(message),
      toolCount: assistantMessages.reduce((total, assistant) => total + assistant.tools.length, 0),
      status: turnStatus(assistantMessages),
      checkpoint: checkpointsByMessageId.get(message.id),
    });
  }
  return turns;
}
