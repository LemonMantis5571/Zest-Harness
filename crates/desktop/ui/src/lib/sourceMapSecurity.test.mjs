import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import test from "node:test";

const require = createRequire(import.meta.url);
const postcssRequire = createRequire(require.resolve("postcss"));
const sourceMapEntry = postcssRequire.resolve("source-map-js");
const { SourceMapConsumer, SourceNode } = postcssRequire("source-map-js");

function indexedMap(line, column = 0) {
  return {
    version: 3,
    sections: [{
      offset: { line, column },
      map: {
        version: 3,
        sources: ["input.js"],
        names: [],
        mappings: "AAAA",
        sourcesContent: ["x"],
      },
    }],
  };
}

test("the build's source-map consumer preserves valid indexed mappings", () => {
  const consumer = new SourceMapConsumer(indexedMap(0));
  const mappings = [];
  consumer.eachMapping((mapping) => mappings.push(mapping));
  assert.deepEqual(mappings, [{
    source: "input.js",
    generatedLine: 1,
    generatedColumn: 0,
    originalLine: 1,
    originalColumn: 0,
    name: null,
  }]);
  assert.equal(consumer.sourceContentFor("input.js"), "x");
  assert.equal(SourceNode.fromStringWithSourceMap("x", consumer).toString(), "x");
});

test("the build's source-map consumer rejects malformed and excessive offsets", () => {
  for (const offset of [Infinity, NaN, 0.5, -1, "0"]) {
    assert.throws(
      () => new SourceMapConsumer(indexedMap(offset)),
      /Section offset line and column must be non-negative integers/,
    );
    assert.throws(
      () => new SourceMapConsumer(indexedMap(0, offset)),
      /Section offset line and column must be non-negative integers/,
    );
  }
  assert.throws(
    () => new SourceMapConsumer(indexedMap(Number.MAX_SAFE_INTEGER)),
    /Section offset line must not exceed/,
  );

  const nested = indexedMap(1);
  nested.sections[0].map = indexedMap(10_000_000);
  assert.throws(
    () => new SourceMapConsumer(nested),
    /Section offset line must not exceed/,
  );
});

test("source-map conversion stops walking lines after the generated input ends", () => {
  const script = `
    const { SourceMapConsumer, SourceNode } = require(${JSON.stringify(sourceMapEntry)});
    const consumer = new SourceMapConsumer(${JSON.stringify(indexedMap(10_000_000))});
    process.stdout.write(SourceNode.fromStringWithSourceMap("x", consumer).toString());
  `;
  const output = execFileSync(process.execPath, ["--eval", script], {
    encoding: "utf8",
    timeout: 5_000,
    stdio: ["ignore", "pipe", "pipe"],
  });
  assert.equal(output, "x");
});
