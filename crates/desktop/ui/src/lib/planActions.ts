import { looksLikeDocument } from "./documentShape.ts";
import { planningQuestionFor } from "./planningQuestion.ts";
import type { ChatMessage } from "./types.ts";

/** The skill whose output Build acts on. */
export const PLAN_COMMAND = "plan";

/**
 * Which plan, if any, should offer a Build button.
 *
 * Only the newest finished plan, and only while it is still the last thing that
 * happened. Two reasons, both about the button meaning what it says:
 *
 * - A button on every plan in a thread would be a lie, because the prompt it
 *   sends says "the plan" and the model reads the conversation, not the button.
 *   With two plans on screen, whichever one you click builds the newer.
 * - Once the conversation moves past a plan, building it silently would act on
 *   something the discussion may already have overtaken.
 *
 * Assistant turns that ran tools without saying anything do not count as moving
 * on — they are how the plan got written.
 */
export function buildablePlanId(messages: ChatMessage[]): string | null {
  for (let i = messages.length - 1; i >= 0; i--) {
    const msg = messages[i];
    if (msg.role === "assistant" && msg.streaming) continue;
    if (
      msg.role === "assistant" &&
      msg.command === PLAN_COMMAND &&
      looksLikeDocument(msg.text) &&
      !planningQuestionFor(msg)
    ) {
      return msg.id;
    }
    if (msg.text.trim()) return null;
  }
  return null;
}
