import { buildablePlanId } from "./planActions.ts";
import { looksLikeDocument } from "./documentShape.ts";
import { planningQuestionFor } from "./planningQuestion.ts";
import type { ChatMessage } from "./types.ts";

function planContext(messages: ChatMessage[], id: string) {
  const index = messages.findIndex((message) => message.id === id);
  if (index < 0) return null;
  const plan = messages[index].text.trim();
  const request = messages.slice(Math.max(0, index - 16), index)
    .filter((message) => message.role === "user" && message.text.trim())
    .map((message) => message.text.trim())
    .join("\n\n")
    .slice(-12_000);
  return request && plan ? { id, request, plan } : null;
}

export function completedPlanContext(messages: ChatMessage[]) {
  const id = buildablePlanId(messages);
  return id ? planContext(messages, id) : null;
}

export function latestFinishedPlanContext(messages: ChatMessage[]) {
  const message = [...messages].reverse().find((item) =>
    item.role === "assistant" && item.command === "plan" && !item.streaming &&
    looksLikeDocument(item.text) && !planningQuestionFor(item)
  );
  return message ? planContext(messages, message.id) : null;
}

export function changeReviewContext(messages: ChatMessage[]) {
  const assistantIndex = messages.findLastIndex((message) => message.role === "assistant" && !message.streaming && message.command !== "plan" && message.text.trim());
  if (assistantIndex < 0) return null;
  const lastUsers = messages.slice(0, assistantIndex).filter((message) => message.role === "user" && message.text.trim()).slice(-3);
  const objective = lastUsers.map((message) => message.text.trim()).join("\n\n").slice(-12_000);
  const claimedSummary = messages[assistantIndex].text.trim().slice(0, 8_000);
  return objective ? { objective, claimedSummary } : null;
}
