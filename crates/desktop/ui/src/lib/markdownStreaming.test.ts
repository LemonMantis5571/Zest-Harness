import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

describe("Markdown streaming fences", () => {
  it("forwards streaming into CodeBlock on the pre path", () => {
    const src = readFileSync(join(here, "../components/Markdown.tsx"), "utf8");
    assert.match(
      src,
      /<CodeBlock code=\{code\} language=\{language\} streaming=\{streaming\} \/>/
    );
  });

  it("renders markdown images through ZoomableImage", () => {
    const src = readFileSync(join(here, "../components/Markdown.tsx"), "utf8");
    assert.match(src, /img: \(\{ src, alt \}\) => <ZoomableImage src=\{src\} alt=\{alt\} \/>/);
    assert.match(src, /hoistLocalImages\(children\)/);
  });
});
