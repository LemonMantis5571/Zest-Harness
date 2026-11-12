import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { parseHiddenPickerProviders } from "./pickerProviders.ts";

describe("parseHiddenPickerProviders", () => {
  it("reads the stored provider ids", () => {
    assert.deepEqual([...parseHiddenPickerProviders('["cursor","codex"]')], ["cursor", "codex"]);
  });

  it("hides nothing when the value is missing or unreadable", () => {
    for (const raw of [null, "", "not json", '{"cursor":true}', "[1, null]"]) {
      assert.equal(parseHiddenPickerProviders(raw).size, 0, String(raw));
    }
  });

  it("drops entries that are not provider ids", () => {
    assert.deepEqual([...parseHiddenPickerProviders('["claude", 7, "", null]')], ["claude"]);
  });
});
