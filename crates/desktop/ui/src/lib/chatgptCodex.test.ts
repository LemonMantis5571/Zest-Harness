import assert from "node:assert/strict";
import test from "node:test";

import { CHATGPT_CODEX_ID, isChatgptCodexRow } from "./chatgptCodex.ts";

test("recognises the ChatGPT Codex sibling and method", () => {
  assert.equal(
    isChatgptCodexRow({ id: CHATGPT_CODEX_ID, method: "ChatGPT sign-in" }),
    true
  );
  assert.equal(
    isChatgptCodexRow({ id: "codex", method: "ChatGPT sign-in" }),
    true
  );
  assert.equal(
    isChatgptCodexRow({ id: "codex", method: "Codex CLI subscription" }),
    false
  );
});
