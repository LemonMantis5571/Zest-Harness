import assert from "node:assert/strict";
import test from "node:test";

import {
  busyTurnMessage,
  conversationRecovery,
  isWorkspaceProblem,
  shouldOfferProviderReconnect,
  workspaceProblemMessage,
} from "./invokeErrors.ts";

/**
 * The shipped bug: a fresh Windows install adopted its own read-only program
 * folder as the project. `ThreadStore::open` failed, and because the classifier
 * only looked for "access denied" while Windows writes "Access is denied", the
 * picker showed "Something went wrong. Try again." under a Codex row that was
 * signed in and perfectly healthy.
 */
test("windows permission phrasing is recognised as a folder problem", () => {
  const windowsError =
    String.raw`create thread dir C:\Program Files\Zest\.zest\threads: Access is denied. (os error 5)`;
  assert.equal(isWorkspaceProblem(windowsError), true);
  assert.equal(
    isWorkspaceProblem("create thread dir /opt/zest/.zest/threads: Permission denied (os error 13)"),
    true
  );
});

test("the backend token wins over phrase matching and carries its own guidance", () => {
  const tagged =
    "workspace_not_writable: Zest cannot save chats in C:/Program Files/Zest. " +
    "Use Open to choose a folder you own, such as one under Documents.";
  assert.equal(isWorkspaceProblem(tagged), true);
  const message = workspaceProblemMessage(tagged);
  assert.ok(!message.includes("workspace_not_writable"), message);
  assert.ok(message.startsWith("Zest cannot save chats in"), message);
});

test("untagged storage failures still get actionable fallback copy", () => {
  const message = workspaceProblemMessage("create run dir /x/.zest/runs: Permission denied");
  assert.ok(message.includes("Use Open"), message);
});

test("provider and unrelated failures are not mistaken for folder problems", () => {
  assert.equal(isWorkspaceProblem("Codex needs to be reconnected. Try again."), false);
  assert.equal(isWorkspaceProblem("This provider is overloaded."), false);
  // A permission word with nothing filesystem about it must not hijack the
  // folder guidance — that would send people to the folder picker for an
  // account problem.
  assert.equal(isWorkspaceProblem("permission denied by the account policy"), false);
});

test("provider reconnect detection follows the desktop error contract", () => {
  assert.equal(
    shouldOfferProviderReconnect("DeepSeek needs to be reconnected. Try again."),
    true
  );
  assert.equal(
    shouldOfferProviderReconnect(
      "{\"code\":\"auth_unavailable\",\"message\":\"No sign-in is available\"}"
    ),
    true
  );
  assert.equal(shouldOfferProviderReconnect("This provider is overloaded."), false);
});

test("provider-owned recovery exposes an explicit copy target", () => {
  const recovery = conversationRecovery(
    JSON.stringify({
      code: "provider_unavailable",
      message: "Codex is not configured for this project.",
      details: {
        threadId: "thread-codex",
        providerId: "codex",
        providerLabel: "Codex",
        configured: false,
        availableProviders: [
          { id: "deepseek", label: "DeepSeek", model: "deepseek-chat" },
        ],
      },
    })
  );

  assert.deepEqual(recovery, {
    kind: "owner_unavailable",
    threadId: "thread-codex",
    providerId: "codex",
    providerLabel: "Codex",
    configured: false,
    providers: [{ id: "deepseek", label: "DeepSeek", model: "deepseek-chat" }],
  });
});

test("legacy recovery requires a provider choice", () => {
  const recovery = conversationRecovery(
    JSON.stringify({
      code: "thread_provider_unknown",
      message: "This chat has no provider owner.",
      details: {
        threadId: "thread-legacy",
        availableProviders: [{ id: "codex", label: "Codex", model: "gpt-5" }],
      },
    })
  );

  assert.deepEqual(recovery, {
    kind: "unknown_owner",
    threadId: "thread-legacy",
    providers: [{ id: "codex", label: "Codex", model: "gpt-5" }],
  });
});

test("new project chats expose provider recovery without a thread id", () => {
  const recovery = conversationRecovery(
    JSON.stringify({
      code: "provider_unavailable",
      message: "Anthropic is not ready for this project.",
      details: {
        threadId: null,
        providerId: "anthropic",
        providerLabel: "Anthropic",
        configured: true,
        availableProviders: [{ id: "deepseek", label: "DeepSeek", model: "deepseek-chat" }],
      },
    })
  );

  assert.deepEqual(recovery, {
    kind: "new_chat_unavailable",
    threadId: null,
    providerId: "anthropic",
    providerLabel: "Anthropic",
    configured: true,
    providers: [{ id: "deepseek", label: "DeepSeek", model: "deepseek-chat" }],
  });
});

test("busy errors keep the backend's next action", () => {
  assert.equal(
    busyTurnMessage(
      JSON.stringify({
        code: "busy",
        message: "this chat is still working — switch chats or wait for it to finish",
      })
    ),
    "This chat is still working — switch chats or wait for it to finish"
  );
  assert.equal(
    busyTurnMessage("already in progress"),
    "This chat is still working. Switch chats or wait for it to finish."
  );
});
