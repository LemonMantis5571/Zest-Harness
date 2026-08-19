import assert from "node:assert/strict";
import test from "node:test";

import {
  AVAILABLE_FONTS,
  DEFAULT_FONT_ID,
  applyFont,
  getFontById,
  getSavedFontId,
} from "./fonts.ts";

test("fonts registry contains ABC Arizona and popular fonts", () => {
  const ids = AVAILABLE_FONTS.map((f) => f.id);
  assert.ok(ids.includes("geist"), "should include geist");
  assert.ok(ids.includes("abc-arizona"), "should include abc-arizona");
  assert.ok(ids.includes("inter"), "should include inter");
  assert.ok(ids.includes("plus-jakarta"), "should include plus-jakarta");
  assert.ok(ids.includes("jetbrains-mono"), "should include jetbrains-mono");
  assert.ok(ids.includes("fira-code"), "should include fira-code");
  assert.ok(ids.includes("system"), "should include system");
});

test("getFontById returns fallback on unknown id", () => {
  const fallback = getFontById("unknown-font");
  assert.equal(fallback.id, DEFAULT_FONT_ID);
});

test("getFontById returns requested font", () => {
  const arizona = getFontById("abc-arizona");
  assert.equal(arizona.name, "ABC Arizona");
  assert.equal(arizona.category, "variable");
});

test("getSavedFontId returns default when storage is empty", () => {
  assert.equal(getSavedFontId(), DEFAULT_FONT_ID);
});

test("applyFont returns matching font object", () => {
  const applied = applyFont("abc-arizona");
  assert.equal(applied.id, "abc-arizona");
  assert.equal(applied.name, "ABC Arizona");
});
