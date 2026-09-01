import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { externalHttpUrlFromClick, safeHttpUrl } from "./externalLinks.ts";

describe("safeHttpUrl", () => {
  it("keeps http and https", () => {
    assert.equal(safeHttpUrl("https://commons.wikimedia.org/wiki/File:x"), "https://commons.wikimedia.org/wiki/File:x");
    assert.equal(safeHttpUrl("http://example.com/a"), "http://example.com/a");
  });

  it("rejects other schemes", () => {
    assert.equal(safeHttpUrl("javascript:alert(1)"), null);
    assert.equal(safeHttpUrl("file:///tmp/x"), null);
    assert.equal(safeHttpUrl("zest://local"), null);
    assert.equal(safeHttpUrl("not a url"), null);
  });
});

describe("externalHttpUrlFromClick", () => {
  it("opens an https anchor", () => {
    const anchor = {
      tagName: "A",
      getAttribute(name: string) {
        return name === "href" ? "https://github.com/zest/pr/18" : null;
      },
      hasAttribute() {
        return false;
      },
    };
    const child = { tagName: "SPAN", parentElement: anchor };
    assert.equal(
      externalHttpUrlFromClick({
        defaultPrevented: false,
        button: 0,
        target: child,
      }),
      "https://github.com/zest/pr/18"
    );
    assert.equal(
      externalHttpUrlFromClick({
        defaultPrevented: false,
        button: 0,
        target: child,
        altKey: true,
      }),
      null
    );
  });

  it("ignores download links", () => {
    const anchor = {
      tagName: "A",
      getAttribute(name: string) {
        return name === "href" ? "https://example.com/file.zip" : null;
      },
      hasAttribute(name: string) {
        return name === "download";
      },
    };
    assert.equal(
      externalHttpUrlFromClick({
        defaultPrevented: false,
        button: 0,
        target: anchor,
      }),
      null
    );
  });

  it("skips non-primary clicks that are not middle-click", () => {
    const anchor = {
      tagName: "A",
      getAttribute() {
        return "https://example.com";
      },
    };
    assert.equal(
      externalHttpUrlFromClick({
        defaultPrevented: false,
        button: 2,
        target: anchor,
      }),
      null
    );
  });
});
