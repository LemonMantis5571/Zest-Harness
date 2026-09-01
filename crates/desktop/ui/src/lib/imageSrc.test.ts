import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { safeImageSrc } from "./imageSrc.ts";

describe("safeImageSrc", () => {
  it("allows raster data URLs and http(s)", () => {
    assert.equal(
      safeImageSrc("data:image/png;base64,aaaa"),
      "data:image/png;base64,aaaa"
    );
    assert.equal(
      safeImageSrc("https://upload.wikimedia.org/wikipedia/commons/x.jpg"),
      "https://upload.wikimedia.org/wikipedia/commons/x.jpg"
    );
  });

  it("rejects svg data URLs and non-image schemes", () => {
    assert.equal(safeImageSrc("data:image/svg+xml;base64,PHN2Zy8+"), null);
    assert.equal(safeImageSrc("data:text/html;base64,PGgxPg=="), null);
    assert.equal(safeImageSrc("javascript:alert(1)"), null);
  });
});
