import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  ATTENTION_KINDS,
  attentionChatName,
  attentionRoute,
  attentionTitle,
  replyExcerpt,
  isLongTurn,
  LONG_TURN_NOTIFICATION_MS,
  mergeToastDescription,
  notificationFingerprint,
  type AttentionKind,
  type AttentionRoute,
} from "./notificationPolicy.ts";

/**
 * Every event × on-screen × focused case, written out so a change to the
 * policy has to change this table too.
 */
const ROUTES: Record<
  AttentionKind,
  {
    onScreenFocused: AttentionRoute;
    onScreenAway: AttentionRoute;
    elsewhereFocused: AttentionRoute;
    elsewhereAway: AttentionRoute;
  }
> = {
  approval_needed: { onScreenFocused: "none", onScreenAway: "os", elsewhereFocused: "toast", elsewhereAway: "os" },
  input_needed: { onScreenFocused: "none", onScreenAway: "os", elsewhereFocused: "toast", elsewhereAway: "os" },
  response_ready: { onScreenFocused: "none", onScreenAway: "os", elsewhereFocused: "toast", elsewhereAway: "os" },
  request_failed: { onScreenFocused: "none", onScreenAway: "os", elsewhereFocused: "toast", elsewhereAway: "os" },
  job_finished: { onScreenFocused: "none", onScreenAway: "os", elsewhereFocused: "none", elsewhereAway: "os" },
  job_failed: { onScreenFocused: "toast", onScreenAway: "os", elsewhereFocused: "toast", elsewhereAway: "os" },
  delegation_ready: { onScreenFocused: "toast", onScreenAway: "os", elsewhereFocused: "toast", elsewhereAway: "os" },
  delegation_changes_requested: { onScreenFocused: "toast", onScreenAway: "os", elsewhereFocused: "toast", elsewhereAway: "os" },
  delegation_blocked: { onScreenFocused: "toast", onScreenAway: "os", elsewhereFocused: "toast", elsewhereAway: "os" },
  delegation_failed: { onScreenFocused: "toast", onScreenAway: "os", elsewhereFocused: "toast", elsewhereAway: "os" },
  delegation_applied: { onScreenFocused: "toast", onScreenAway: "os", elsewhereFocused: "toast", elsewhereAway: "os" },
  delegation_cancelled: { onScreenFocused: "toast", onScreenAway: "os", elsewhereFocused: "toast", elsewhereAway: "os" },
};

describe("attention routes", () => {
  it("covers every attention kind", () => {
    assert.deepEqual(Object.keys(ROUTES).sort(), [...ATTENTION_KINDS].sort());
  });

  for (const kind of ATTENTION_KINDS) {
    const expected = ROUTES[kind];
    it(`routes ${kind}`, () => {
      assert.equal(attentionRoute(kind, { onScreen: true, focused: true }), expected.onScreenFocused, "on screen, focused");
      assert.equal(attentionRoute(kind, { onScreen: true, focused: false }), expected.onScreenAway, "on screen, away");
      assert.equal(attentionRoute(kind, { onScreen: false, focused: true }), expected.elsewhereFocused, "elsewhere, focused");
      assert.equal(attentionRoute(kind, { onScreen: false, focused: false }), expected.elsewhereAway, "elsewhere, away");
    });

    it(`says nothing about ${kind} when the user's own click caused it`, () => {
      for (const onScreen of [true, false]) {
        for (const focused of [true, false]) {
          assert.equal(attentionRoute(kind, { onScreen, focused, byUser: true }), "none");
        }
      }
    });
  }
});

describe("attention copy", () => {
  it("names the chat by title, then first prompt, then the sidebar fallback", () => {
    assert.equal(attentionChatName("  Fix login  ", "ignored"), "Fix login");
    assert.equal(attentionChatName(undefined, "Why does\n  the build fail?"), "Why does the build fail?");
    assert.equal(attentionChatName("", "  "), "Untitled chat");
    const long = attentionChatName("A".repeat(80), undefined);
    assert.equal(long.length, 48);
    assert.ok(long.endsWith("…"));
  });

  it("puts the chat after the event", () => {
    assert.equal(attentionTitle("Approval needed", "Fix login"), "Approval needed · Fix login");
  });

  it("turns the start of a reply into plain text", () => {
    assert.equal(
      replyExcerpt("## Done\n\n- Fixed **the** `redirect`\n- See [the PR](https://example.test/1)"),
      "Done Fixed the redirect See the PR"
    );
    assert.equal(replyExcerpt("Before\n```ts\nconst x = 1;\n```\nafter"), "Before after");
    assert.equal(replyExcerpt("keep snake_case names"), "keep snake_case names");
    const long = replyExcerpt("word ".repeat(60));
    assert.equal(long.length, 140);
    assert.ok(long.endsWith("…"));
  });
});

describe("notification policy", () => {
  it("only marks turns at or above the long-turn threshold", () => {
    assert.equal(isLongTurn(LONG_TURN_NOTIFICATION_MS - 1), false);
    assert.equal(isLongTurn(LONG_TURN_NOTIFICATION_MS), true);
    assert.equal(isLongTurn(LONG_TURN_NOTIFICATION_MS + 1), true);
  });

  it("groups only notifications with the same visible content", () => {
    const first = notificationFingerprint(
      "error",
      "Could not allow tool",
      "That request expired."
    );
    assert.equal(
      notificationFingerprint("error", "Could not allow tool", "That request expired."),
      first
    );
    assert.notEqual(
      notificationFingerprint("warning", "Could not allow tool", "That request expired."),
      first
    );
    assert.notEqual(
      notificationFingerprint("error", "Could not allow tool", "Try again."),
      first
    );
  });

  it("groups success toasts by title so extra copy does not stack a second card", () => {
    const deleted = notificationFingerprint("success", "Chat deleted", "");
    assert.equal(
      notificationFingerprint(
        "success",
        "Chat deleted",
        "Type to start a new chat"
      ),
      deleted
    );
    assert.notEqual(
      notificationFingerprint("success", "Fork created", ""),
      deleted
    );
  });

  it("keeps a hint when collapsing success toasts", () => {
    assert.equal(
      mergeToastDescription("", "Type to start a new chat"),
      "Type to start a new chat"
    );
    assert.equal(
      mergeToastDescription("Type to start a new chat", ""),
      "Type to start a new chat"
    );
  });
});
