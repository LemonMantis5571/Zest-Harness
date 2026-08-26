import assert from "node:assert/strict";
import test from "node:test";

import { makeReadingDiff } from "./readingDiff.ts";

test("hides import churn but keeps diff structure and behavior", () => {
  const result = makeReadingDiff(
    [
      "diff --git a/src/a.ts b/src/a.ts",
      "--- a/src/a.ts",
      "+++ b/src/a.ts",
      "@@ -1,7 +1,7 @@",
      " import { oldThing } from \"old\";",
      "-import { noisy } from \"noisy\";",
      "+import { useful } from \"useful\";",
      " const value = 1;",
      "-return oldThing(value);",
      "+return useful(value);",
    ].join("\n")
  );

  assert.equal(result.hiddenImports, 3);
  assert.match(result.diff, /@@ -1,7 \+1,7 @@/);
  assert.match(result.diff, /-return oldThing\(value\);/);
  assert.match(result.diff, /\+return useful\(value\);/);
  assert.doesNotMatch(result.diff, /noisy/);
});

test("folds only long unchanged context runs", () => {
  const result = makeReadingDiff(
    [
      "@@ -1,8 +1,8 @@",
      " one",
      " two",
      " three",
      " four",
      " five",
      " six",
      "+changed",
    ].join("\n")
  );

  assert.equal(result.foldedContextLines, 4);
  assert.match(result.diff, / …/);
  assert.match(result.diff, /\+changed/);
});
