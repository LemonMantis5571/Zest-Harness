import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { isBooleanRecord, isRecord, parseJson } from "./json.ts";

describe("JSON boundary helpers", () => {
  it("returns unknown data without throwing on malformed input", () => {
    assert.deepEqual(parseJson('{"enabled":true}'), { enabled: true });
    assert.equal(parseJson("not json"), null);
  });

  it("recognizes records but rejects arrays and null", () => {
    assert.equal(isRecord({ key: "value" }), true);
    assert.equal(isRecord(["value"]), false);
    assert.equal(isRecord(null), false);
  });

  it("accepts only boolean maps for sidebar state", () => {
    assert.equal(isBooleanRecord({ project: true, other: false }), true);
    assert.equal(isBooleanRecord({ project: "true" }), false);
    assert.equal(isBooleanRecord({}), true);
  });
});
