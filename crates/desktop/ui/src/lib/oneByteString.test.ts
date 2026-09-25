import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { serialize } from "node:v8";

import { toOneByteIfLatin1 } from "./oneByteString.ts";

/** V8's serializer tags a string `"` when it is stored one byte per char and
 *  `c` when two, after the header and any alignment padding. */
function storage(text: string): "one-byte" | "two-byte" {
  const bytes = serialize(text);
  let i = 1;
  while (bytes[i] & 0x80) i++;
  i++;
  while (bytes[i] === 0) i++;
  return bytes[i] === 0x22 ? "one-byte" : "two-byte";
}

const code = 'const url = `/api/${id}`; // "quoted"\n'.repeat(40);

describe("toOneByteIfLatin1", () => {
  it("turns a Latin-1 slice of a two-byte reply back into one-byte text", () => {
    const reply = `Here is the fix — it retries.\n\n${code}`;
    const block = reply.slice(reply.length - code.length);
    assert.equal(block, code);
    assert.equal(storage(block), "two-byte", "the slice inherits its parent's storage");

    const copy = toOneByteIfLatin1(block);
    assert.equal(copy, code);
    assert.equal(storage(copy), "one-byte");
  });

  it("keeps Latin-1 accents and copies long text in chunks", () => {
    const accented = "café naïve über\n".repeat(2_000);
    const block = (`—${accented}`).slice(1);
    const copy = toOneByteIfLatin1(block);
    assert.equal(copy, accented);
    assert.equal(storage(copy), "one-byte");
  });

  it("returns text that cannot be one-byte unchanged", () => {
    for (const text of ["a — b", "emoji \u{1F600}", "Δ"]) {
      assert.equal(toOneByteIfLatin1(text), text);
    }
    assert.equal(toOneByteIfLatin1(""), "");
  });
});
