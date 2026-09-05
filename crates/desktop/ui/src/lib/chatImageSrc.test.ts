import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { resolveChatImageSrc } from "./chatImageSrc.ts";

describe("resolveChatImageSrc", () => {
  it("keeps http and raster data URLs", () => {
    assert.equal(
      resolveChatImageSrc("https://example.com/x.png"),
      "https://example.com/x.png"
    );
    assert.equal(
      resolveChatImageSrc("data:image/png;base64,aaaa"),
      "data:image/png;base64,aaaa"
    );
  });

  it("does not invent a src for a local path outside Tauri", () => {
    assert.equal(resolveChatImageSrc("/tmp/frutiger-aero.png"), null);
    assert.equal(
      resolveChatImageSrc(
        "C:/Users/brite/AppData/Local/Temp/frutiger-aero.png"
      ),
      null
    );
  });
});
