import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  hoistLocalImages,
  localImageBasename,
  localImagePathsIn,
  parseLocalImagePath,
  toFileImageUrl,
} from "./localImagePath.ts";

describe("parseLocalImagePath", () => {
  it("accepts a Windows raster path", () => {
    const path = parseLocalImagePath(
      String.raw`C:\Users\brite\AppData\Local\Temp\frutiger-aero.png`
    );
    assert.equal(
      path,
      "C:/Users/brite/AppData/Local/Temp/frutiger-aero.png"
    );
  });

  it("accepts a Unix raster path and a file URL", () => {
    assert.equal(parseLocalImagePath("/tmp/frutiger-aero.webp"), "/tmp/frutiger-aero.webp");
    assert.equal(
      parseLocalImagePath(
        "file:///C:/Users/brite/AppData/Local/Temp/frutiger-aero.png"
      ),
      "C:/Users/brite/AppData/Local/Temp/frutiger-aero.png"
    );
  });

  it("unwraps a code span", () => {
    assert.equal(
      parseLocalImagePath("`/tmp/shot.jpg`"),
      "/tmp/shot.jpg"
    );
  });

  it("rejects relatives, traversal, svg, and remote URLs", () => {
    assert.equal(parseLocalImagePath("shot.png"), null);
    assert.equal(parseLocalImagePath("./shot.png"), null);
    assert.equal(parseLocalImagePath("/tmp/../secret.png"), null);
    assert.equal(parseLocalImagePath("/tmp/note.svg"), null);
    assert.equal(parseLocalImagePath("https://example.com/shot.png"), null);
    assert.equal(parseLocalImagePath("s://example.com/shot.png"), null);
    assert.equal(parseLocalImagePath("javascript:alert(1)"), null);
  });
});

describe("localImagePathsIn", () => {
  it("finds a Windows path inside a longer code span", () => {
    const text =
      String.raw`C:\Users\brite\AppData\Local\Temp\frutiger-aero.png — 1920×1200 PNG, 11.5 MB, still on disk from the earlier download.`;
    assert.deepEqual(localImagePathsIn(text), [
      "C:/Users/brite/AppData/Local/Temp/frutiger-aero.png",
    ]);
  });

  it("does not treat an https image as local", () => {
    assert.deepEqual(
      localImagePathsIn("see https://example.com/hero.png please"),
      []
    );
  });
});

describe("hoistLocalImages", () => {
  it("inserts a markdown image above a local path in prose", () => {
    const message = [
      "Here it is, displayed inline.",
      "",
      "`C:\\Users\\brite\\AppData\\Local\\Temp\\frutiger-aero.png` — 1920×1200 PNG.",
    ].join("\n");
    const hoisted = hoistLocalImages(message);
    assert.match(
      hoisted,
      /!\[frutiger-aero\.png\]\(file:\/\/\/C:\/Users\/brite\/AppData\/Local\/Temp\/frutiger-aero\.png\)/
    );
    assert.ok(hoisted.includes("Here it is, displayed inline."));
    assert.ok(hoisted.includes("1920×1200 PNG"));
  });

  it("leaves fenced code and existing remote images alone", () => {
    const fenced = ["```", "/tmp/hidden.png", "```"].join("\n");
    assert.equal(hoistLocalImages(fenced), fenced);

    const remote = "![hero](https://example.com/hero.png)";
    assert.equal(hoistLocalImages(remote), remote);
  });

  it("does not duplicate a path already used as a markdown image", () => {
    const line = "![shot](/tmp/shot.png)";
    assert.equal(
      hoistLocalImages(line),
      "![shot](file:///tmp/shot.png)"
    );
  });
});

describe("toFileImageUrl", () => {
  it("uses three slashes on Windows and two-plus-root on Unix", () => {
    const win = parseLocalImagePath("D:/Code/aero.png");
    const unix = parseLocalImagePath("/tmp/aero.png");
    assert.ok(win);
    assert.ok(unix);
    assert.equal(toFileImageUrl(win), "file:///D:/Code/aero.png");
    assert.equal(toFileImageUrl(unix), "file:///tmp/aero.png");
    assert.equal(localImageBasename(win), "aero.png");
  });
});
