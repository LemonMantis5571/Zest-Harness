/** Sibling id when `[providers.codex]` is already a different kind. */
export const CHATGPT_CODEX_ID = "codex-chatgpt";

/** ChatGPT Codex is a user choice, not a fallback for a missing CLI. */
export function isChatgptCodexRow(row: { id: string; method: string }): boolean {
  return row.method === "ChatGPT sign-in" || row.id === CHATGPT_CODEX_ID;
}
