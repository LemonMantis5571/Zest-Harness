import assert from "node:assert/strict";
import { before, describe, it } from "node:test";

import { createHighlighterCore } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";

import { createResumableHighlight } from "./resumableHighlight.ts";

type Highlighter = Awaited<ReturnType<typeof createHighlighterCore>>;
type State = NonNullable<ReturnType<Highlighter["codeToTokens"]>["grammarState"]>;

// Constructs whose colour depends on earlier lines: a block comment, a
// template literal and a JSDoc block that each span lines, plus a regex.
const SNIPPET = [
  "/* A block comment",
  "   spanning lines with `code` inside */",
  "const greeting = `hello",
  "${name}, still the template`; // trailing comment",
  "function pick(a: number): string {",
  '  return a > 1 ? "big" : \'small\'',
  "}",
  "/** Docs",
  " * @param value the thing",
  " */",
  "export class Box<T> { constructor(private value: T) {} }",
  "const re = /ab+c/gi;",
  "",
].join("\n");

let highlighter: Highlighter;
before(async () => {
  highlighter = await createHighlighterCore({
    engine: createJavaScriptRegexEngine({ forgiving: true }),
    langs: [import("shiki/langs/typescript.mjs")],
    themes: [import("shiki/themes/github-light.mjs"), import("shiki/themes/github-dark.mjs")],
  });
});

function tokenizer(counter?: { chars: number }) {
  return (text: string, state: State | undefined) => {
    if (counter) counter.chars += text.length;
    const result = highlighter.codeToTokens(text, {
      lang: "typescript",
      themes: { light: "github-light", dark: "github-dark" },
      ...(state ? { grammarState: state } : {}),
    });
    if (!result.grammarState) return null;
    // Offsets are relative to the tokenized text, so compare content and style.
    const lines = result.tokens.map((line) =>
      JSON.stringify(line.map((token) => [token.content, token.htmlStyle ?? token.color]))
    );
    return { lines, state: result.grammarState };
  };
}

/** Prefixes of uneven sizes, so chunks end mid-token, mid-line and on "\n". */
function streamedPrefixes(text: string): string[] {
  const prefixes: string[] = [];
  let end = 0;
  for (let step = 0; end < text.length; step++) {
    end = Math.min(text.length, end + 1 + (step % 9));
    prefixes.push(text.slice(0, end));
  }
  return prefixes;
}

describe("createResumableHighlight", () => {
  it("matches a full pass on every streamed prefix", () => {
    const resume = createResumableHighlight(tokenizer());
    const full = tokenizer();
    for (const prefix of streamedPrefixes(SNIPPET)) {
      assert.deepEqual(resume(prefix), full(prefix, undefined)?.lines, JSON.stringify(prefix));
    }
  });

  it("would catch a resume that dropped the grammar state", () => {
    // Control for the test above: the multi-line constructs really do depend
    // on the carried state, so ignoring it must change the colours.
    const stateless = tokenizer();
    const resume = createResumableHighlight((text: string) => stateless(text, undefined));
    const diverged = streamedPrefixes(SNIPPET).some(
      (prefix) =>
        JSON.stringify(resume(prefix)) !== JSON.stringify(tokenizer()(prefix, undefined)?.lines)
    );
    assert.ok(diverged);
  });

  it("tokenizes each settled line once instead of the whole block per chunk", () => {
    const counter = { chars: 0 };
    const resume = createResumableHighlight(tokenizer(counter));
    const prefixes = streamedPrefixes(SNIPPET.repeat(8));
    for (const prefix of prefixes) resume(prefix);
    const fullPassChars = prefixes.reduce((sum, prefix) => sum + prefix.length, 0);
    // Settled text once, plus each chunk's unfinished line again.
    assert.ok(
      counter.chars < fullPassChars / 10,
      `${counter.chars} chars tokenized; a full pass per chunk would be ${fullPassChars}`
    );
  });

  it("starts over when the text is rewritten rather than extended", () => {
    const resume = createResumableHighlight(tokenizer());
    resume("const a = `one\nstill template\n");
    const rewritten = "let b = 2\nconst c = 3";
    assert.deepEqual(resume(rewritten), tokenizer()(rewritten, undefined)?.lines);
  });

  it("gives up rather than guessing when the tokenizer cannot resume", () => {
    const resume = createResumableHighlight<string, number>(() => null);
    assert.equal(resume("a\nb"), null);
  });
});
