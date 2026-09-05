import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const composer = readFileSync(join(here, "../components/Composer.tsx"), "utf8");

describe("composer attachments", () => {
  it("opens attached images in the same lightbox as chat images", () => {
    assert.match(composer, /AttachmentTrigger/);
    assert.match(composer, /<ImageLightbox/);
    assert.match(composer, /setLightbox\(\{ src: preview, alt: att\.name \}\)/);
  });
});

describe("composer send", () => {
  it("clears the local draft before submit so type-then-enter cannot leave a duplicate", () => {
    const start = composer.indexOf("const handleSend");
    const send = composer.slice(start, start + 600);
    assert.match(send, /const sent = textRef\.current/);
    assert.match(send, /setText\(""\)/);
    assert.match(send, /flushChange\(""\)/);
    assert.match(send, /onSubmit\(sent\)/);
    assert.doesNotMatch(send, /flushChange\(textRef\.current\)/);
  });
});
