import assert from "node:assert/strict";
import test from "node:test";

import {
  AVAILABLE_THEMES,
  DEFAULT_THEME_ID,
  applyTheme,
  blobatarToneFor,
  getSavedThemeId,
  getThemeById,
} from "./themes.ts";

test("theme registry includes Zest, Nights, and Oceanic", () => {
  const ids = AVAILABLE_THEMES.map((theme) => theme.id);
  assert.ok(ids.includes("zest"));
  assert.ok(ids.includes("nights"));
  assert.ok(ids.includes("oceanic"));
  assert.equal(DEFAULT_THEME_ID, "zest");
});

test("Zest and Nights are dark, Oceanic is light", () => {
  assert.equal(getThemeById("zest").appearance, "dark");
  assert.equal(getThemeById("nights").appearance, "dark");
  assert.equal(getThemeById("oceanic").appearance, "light");
});

test("getThemeById returns fallback on unknown id", () => {
  const fallback = getThemeById("unknown-theme");
  assert.equal(fallback.id, DEFAULT_THEME_ID);
});

test("getSavedThemeId returns default when storage is empty", () => {
  assert.equal(getSavedThemeId(), DEFAULT_THEME_ID);
});

test("applyTheme returns matching theme object", () => {
  const oceanic = applyTheme("oceanic");
  assert.equal(oceanic.id, "oceanic");
  assert.equal(oceanic.name, "Oceanic");
  const nights = applyTheme("nights");
  assert.equal(nights.id, "nights");
  assert.equal(nights.name, "Nights");
});

test("blobatar tone is light heads on dark, dark heads on light", () => {
  assert.ok(blobatarToneFor("dark") < 0.5);
  assert.ok(blobatarToneFor("light") > 0.5);
});
