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
