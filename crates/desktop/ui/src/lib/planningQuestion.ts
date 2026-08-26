import { looksLikeDocument } from "./documentShape.ts";
import type { ChatMessage } from "./types.ts";

const MAX_INTERACTIVE_QUESTION_LENGTH = 700;
const MAX_CHOICES = 8;
const CHOICE_LINE = /^\s*(?:[-*+]|\d{1,2}[.)])\s+(.+?)\s*$/;
const QUESTION_START = /^(?:what|which|where|when|who|why|how|would|could|can|should|do|does|is|are|pick|choose|select)\b/i;

export type PlanningQuestionChoice = {
  value: string;
  label: string;
};

export type PlanningQuestion = {
  /** Present when the model invoked the interactive ask_user tool. */
  questionId?: string;
  toolCallId?: string;
  prompt: string;
  choices: PlanningQuestionChoice[];
  multiple?: boolean;
  placeholder?: string;
};

function stripInlineMarkdown(value: string): string {
  return value
    .trim()
    .replace(/^#{1,6}\s+/, "")
    .replace(/^\*\*(.*?)\*\*$/s, "$1")
    .replace(/^__(.*?)__$/s, "$1")
    .replace(/^`(.*?)`$/s, "$1")
    .replace(/^\[(.*?)\]\([^)]*\)$/, "$1")
    .trim();
}

function plainText(lines: string[]): string {
  return lines
    .map(stripInlineMarkdown)
    .filter(Boolean)
    .join(" ")
    .replace(/\s+/g, " ")
    .trim();
}

function isQuestionLike(prompt: string, text: string): boolean {
  return (
    text.includes("?") ||
    QUESTION_START.test(prompt) ||
    /:\s*$/.test(prompt)
  );
}

function choicesAt(lines: string[], start: number): PlanningQuestionChoice[] {
  const choices: PlanningQuestionChoice[] = [];
  const seen = new Set<string>();

  for (let index = start; index < lines.length; index += 1) {
    const line = lines[index].trim();
    if (!line) continue;
    const match = CHOICE_LINE.exec(line);
    if (!match) break;

    const label = stripInlineMarkdown(match[1]).replace(/^\[[ xX]\]\s*/, "");
    if (!label) break;
    const value = label.toLowerCase();
    if (seen.has(value)) break;
    seen.add(value);
    choices.push({ value: label, label });
    if (choices.length > MAX_CHOICES) break;
  }

  return choices;
}

/**
 * Returns a compact questionnaire only for a finished, plan-tagged question.
 * A document-shaped plan remains a document even when it contains a question.
 */
export function planningQuestionFor(message: ChatMessage): PlanningQuestion | null {
  if (
    message.role !== "assistant" ||
    message.command !== "plan" ||
    message.streaming
  ) {
    return null;
  }

  const text = message.text.trim();
  if (
    !text ||
    text.length > MAX_INTERACTIVE_QUESTION_LENGTH
  ) {
    return null;
  }

  const documentShaped = looksLikeDocument(text);
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  for (let index = 0; index < lines.length; index += 1) {
    if (!CHOICE_LINE.test(lines[index])) continue;
    const choices = choicesAt(lines, index);
    if (choices.length < 2 || choices.length > MAX_CHOICES) continue;

    const prompt = plainText(lines.slice(0, index));
    if (prompt && isQuestionLike(prompt, prompt)) {
      return { prompt, choices };
    }
  }

  if (documentShaped) return null;
  const prompt = plainText(lines);
  if (!prompt || !isQuestionLike(prompt, text)) return null;
  return { prompt, choices: [] };
}
