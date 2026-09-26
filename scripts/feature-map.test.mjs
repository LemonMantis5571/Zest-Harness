import assert from "node:assert/strict";
import test from "node:test";

import {
  compilePattern,
  findUntouchedDocs,
  isCovered,
  loadFeatures,
  parseFrontmatter,
  renderIndex,
  validateFeatureMap,
} from "./feature-map.mjs";

const doc = (frontmatter) => `---\n${frontmatter}\n---\n\n# Body\n`;

function feature(overrides) {
  return {
    slug: "demo",
    doc: "docs/features/demo.md",
    title: "Demo",
    summary: "Does a thing.",
    paths: [],
    tests: [],
    verify: [],
    ...overrides,
  };
}

test("parses scalars, lists, and quoted values", () => {
  const { data } = parseFrontmatter(
    doc(
      [
        'title: "Model picker"',
        "summary: Pick a model.",
        "paths:",
        "  - crates/core/src/provider/",
        "  - 'crates/desktop/ui/src/lib/model*.ts'",
        "tests:",
        "  - crates/desktop/ui/e2e/model-picker.spec.ts",
      ].join("\n"),
    ),
  );

  assert.equal(data.title, "Model picker");
  assert.deepEqual(data.paths, ["crates/core/src/provider/", "crates/desktop/ui/src/lib/model*.ts"]);
  assert.deepEqual(data.tests, ["crates/desktop/ui/e2e/model-picker.spec.ts"]);
  assert.deepEqual(data.verify, []);
});

test("accepts CRLF line endings", () => {
  const { data } = parseFrontmatter("---\r\ntitle: A\r\nsummary: B\r\npaths:\r\n  - x.rs\r\n---\r\n");
  assert.deepEqual(data.paths, ["x.rs"]);
});

test("rejects unknown keys, missing paths, and stray list items", () => {
  assert.throws(() => parseFrontmatter(doc("title: A\nsummary: B\nowner: me\npaths:\n  - x.rs")), /unknown key "owner"/);
  assert.throws(() => parseFrontmatter(doc("title: A\nsummary: B")), /"paths" must list at least one file/);
  assert.throws(() => parseFrontmatter(doc("  - x.rs\ntitle: A\nsummary: B")), /list item outside a list key/);
  assert.throws(() => parseFrontmatter(doc("title: A\nsummary: B\npaths: x.rs")), /must be a list/);
  assert.throws(() => parseFrontmatter("# no frontmatter\n"), /must start with a --- frontmatter block/);
});

test("patterns match exact files, directory prefixes, and globs", () => {
  assert.ok(compilePattern("crates/core/src/lib.rs")("crates/core/src/lib.rs"));
  assert.ok(!compilePattern("crates/core/src/lib.rs")("crates/core/src/lib.rs.bak"));

  const dir = compilePattern("crates/core/src/provider/");
  assert.ok(dir("crates/core/src/provider/mod.rs"));
  assert.ok(!dir("crates/core/src/providers.rs"));

  const single = compilePattern("crates/desktop/ui/src/lib/model*.ts");
  assert.ok(single("crates/desktop/ui/src/lib/models.ts"));
  assert.ok(!single("crates/desktop/ui/src/lib/nested/models.ts"));

  const deep = compilePattern("crates/**/*.spec.ts");
  assert.ok(deep("crates/desktop/ui/e2e/btw.spec.ts"));
  assert.ok(!deep("crates/desktop/ui/e2e/btw.spec.tsx"));

  assert.ok(compilePattern("crates\\core\\src\\lib.rs")("crates/core/src/lib.rs"));
});

test("coverage applies to source under crates/ and scripts/, minus generated bindings", () => {
  assert.ok(isCovered("crates/core/src/lib.rs"));
  assert.ok(isCovered("scripts/dev-verify.mjs"));
  assert.ok(!isCovered("crates/desktop/ui/src/lib/generated/ChatEvent.ts"));
  assert.ok(!isCovered("crates/desktop/icons/icon.png"));
  assert.ok(!isCovered("docs/SERVE.md"));
});

test("reports stale patterns and unowned source files", () => {
  const files = ["crates/core/src/a.rs", "crates/core/src/b.rs", "crates/core/tests/a_e2e.rs", "README.md"];
  const errors = validateFeatureMap(
    [feature({ paths: ["crates/core/src/a.rs", "crates/core/src/gone.rs"], tests: ["crates/core/tests/a_e2e.rs"] })],
    files,
  );

  assert.equal(errors.length, 2);
  assert.match(errors[0], /"crates\/core\/src\/gone.rs" matches no file/);
  assert.match(errors[1], /^crates\/core\/src\/b.rs: not listed by any feature/);
});

test("a clean map produces no errors", () => {
  const files = ["crates/core/src/a.rs", "crates/desktop/ui/src/lib/generated/X.ts"];
  assert.deepEqual(validateFeatureMap([feature({ paths: ["crates/core/src/"] })], files), []);
});

test("notes features whose code changed without their doc", () => {
  const tools = feature({ slug: "tools", doc: "docs/features/tools.md", paths: ["crates/core/src/tools/"] });
  const chat = feature({ slug: "chat", doc: "docs/features/chat.md", paths: ["crates/core/src/agent.rs"] });

  const notes = findUntouchedDocs(
    [tools, chat],
    ["crates/core/src/tools/bash.rs", "crates/core/src/tools/grep.rs", "crates/core/src/agent.rs", "docs/features/chat.md"],
  );

  assert.deepEqual(
    notes.map(({ feature: owner, touched }) => [owner.slug, touched.length]),
    [["tools", 2]],
  );
});

test("index lists every feature with its checks and escapes table pipes", () => {
  const index = renderIndex([
    feature({ slug: "a", title: "A", summary: "x | y", tests: ["t1", "t2"], verify: ["cmd"] }),
    feature({ slug: "b", title: "B", summary: "untested" }),
  ]);

  assert.match(index, /\| \[A\]\(a\.md\) \| x \\\| y \| 2 test files, 1 command \|/);
  assert.match(index, /\| \[B\]\(b\.md\) \| untested \| \*\*none\*\* \|/);
});

test("loads entries in slug order and skips the README", () => {
  const files = {
    "docs/features/zeta.md": doc("title: Z\nsummary: z\npaths:\n  - z.rs"),
    "docs/features/alpha.md": doc("title: A\nsummary: a\npaths:\n  - a.rs"),
    "docs/features/README.md": "# Feature map\n",
  };
  const features = loadFeatures(
    () => ["zeta.md", "README.md", "alpha.md", "notes.txt"],
    (file) => files[file],
  );

  assert.deepEqual(
    features.map((entry) => [entry.slug, entry.doc]),
    [
      ["alpha", "docs/features/alpha.md"],
      ["zeta", "docs/features/zeta.md"],
    ],
  );
});
