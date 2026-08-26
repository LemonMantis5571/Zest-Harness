import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  admitAttachments,
  decodedLength,
  MAX_IMAGES,
  MAX_TOTAL_IMAGE_BYTES,
} from "./attachmentLimits.ts";
import type { PreparedAttachment } from "./types.ts";

/**
 * Base64 that decodes to exactly `bytes`, padding included.
 *
 * Exact rather than approximate: the limits are compared against these sizes, so
 * a helper that rounds up would make a boundary test assert the wrong boundary.
 */
function base64Of(bytes: number): string {
  const groups = Math.floor(bytes / 3);
  const remainder = bytes % 3;
  if (remainder === 0) return "A".repeat(groups * 4);
  const padding = remainder === 1 ? 2 : 1;
  return "A".repeat(groups * 4 + (4 - padding)) + "=".repeat(padding);
}

/** An image whose base64 decodes to exactly `bytes`. */
function image(name: string, bytes: number): PreparedAttachment {
  return {
    id: name,
    name,
    path: name,
    kind: "image",
    status: "done",
    detail: `${bytes} B`,
    content: null,
    mediaType: "image/png",
    dataBase64: base64Of(bytes),
  } as PreparedAttachment;
}

function text(name: string): PreparedAttachment {
  return {
    id: name,
    name,
    path: name,
    kind: "text",
    status: "done",
    detail: "text",
    content: "hello",
  } as PreparedAttachment;
}

describe("decodedLength", () => {
  it("matches the real decoded size across padding cases", () => {
    // Padding is what makes the naive length/4*3 wrong, by up to two bytes.
    assert.equal(decodedLength("QQ=="), 1);
    assert.equal(decodedLength("QUE="), 2);
    assert.equal(decodedLength("QUFB"), 3);
    assert.equal(decodedLength(""), 0);
    assert.equal(decodedLength(undefined), 0);
    assert.equal(decodedLength(null), 0);
  });
});

describe("admitAttachments", () => {
  it("admits images until the count is reached, keeping the earliest", () => {
    const existing = [image("old.png", 10)];
    const incoming = Array.from({ length: MAX_IMAGES }, (_, i) =>
      image(`new${i}.png`, 10)
    );

    const { accepted, rejected } = admitAttachments(existing, incoming);
    assert.equal(accepted.length, MAX_IMAGES - 1, "one slot was already taken");
    assert.equal(accepted[0].name, "new0.png", "order is preserved");
    assert.equal(rejected.length, 1);
    assert.match(rejected[0].reason, /Only 8 images/);
  });

  it("admits images until the total size is reached", () => {
    // The case a per-file limit does nothing about: each one is legal alone.
    const half = MAX_TOTAL_IMAGE_BYTES / 2;
    const { accepted, rejected } = admitAttachments(
      [],
      [image("a.png", half), image("b.png", half), image("c.png", 1024)]
    );

    assert.deepEqual(
      accepted.map((a) => a.name),
      ["a.png", "b.png"]
    );
    assert.equal(rejected.length, 1);
    assert.match(rejected[0].reason, /16 MB in total/);
  });

  it("counts what is already attached, not just the incoming batch", () => {
    // Ten separate single-file picks must not add up past the ceiling.
    const existing = [image("big.png", MAX_TOTAL_IMAGE_BYTES - 100)];
    const { accepted, rejected } = admitAttachments(existing, [image("next.png", 4096)]);
    assert.equal(accepted.length, 0);
    assert.equal(rejected.length, 1);
  });

  it("never rejects a non-image", () => {
    // Text is bounded by its own character cap and is not what makes a payload
    // large; refusing a pasted file because eight screenshots came first would
    // be nonsense.
    const existing = Array.from({ length: MAX_IMAGES }, (_, i) => image(`i${i}.png`, 10));
    const { accepted, rejected } = admitAttachments(existing, [text("notes.md")]);
    assert.equal(accepted.length, 1);
    assert.equal(rejected.length, 0);
  });

  it("ignores failed attachments when measuring what is attached", () => {
    // A refusal carries no payload, so it must not consume budget.
    const failed = { ...image("bad.png", 10), status: "error" } as PreparedAttachment;
    const { accepted } = admitAttachments([failed], [image("good.png", 10)]);
    assert.equal(accepted.length, 1);
  });
});
