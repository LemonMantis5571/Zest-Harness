import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";

const require = createRequire(import.meta.url);
const rendererRequire = createRequire(require.resolve("mermaid"));
const katex = rendererRequire("katex");

test("Mermaid's math renderer rejects inherited trust permissions", () => {
  const expression = String.raw`\href{https://example.invalid/untrusted-math}{x}`;
  const trusted = katex.renderToString(expression, { trust: true });
  assert.match(trusted, /href="https:\/\/example\.invalid\/untrusted-math"/);

  const inherited = katex.renderToString(expression, Object.create({ trust: true }));
  assert.doesNotMatch(inherited, /href="https:\/\/example\.invalid\/untrusted-math"/);
  assert.match(inherited, /<mtext>\\href<\/mtext>/);

  const math = katex.renderToString("E = mc^2");
  assert.match(math, /class="katex"/);
  assert.match(math, /<mi>E<\/mi>/);
});
