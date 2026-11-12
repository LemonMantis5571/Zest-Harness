import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { HTML_CSP, MAX_HTML_BYTES, htmlDocumentFromFence, htmlDocumentFromMetadata, htmlFilename, isolatedHtml, parseHtmlDocument } from "./htmlArtifacts.ts";
import { hoistLocalImages } from "./localImagePath.ts";
import { splitRenderableBlocks } from "./markdownBlocks.ts";

describe("HTML artifact boundaries", () => {
  it("retains the original document and enforces UTF-8 limits", () => {
    const document = { title: "Counter", html: " <button>+</button>\n" };
    assert.deepEqual(parseHtmlDocument(document), document);
    assert.deepEqual(parseHtmlDocument({ title: "  Counter  ", html: "x" }), { title: "Counter", html: "x" });
    assert.equal([...parseHtmlDocument({ title: "🟢".repeat(120), html: "x" }).title].length, 120);
    assert.equal(parseHtmlDocument({ title: "x", html: "x".repeat(MAX_HTML_BYTES) }).html.length, MAX_HTML_BYTES);
    for (const value of [null, {}, { title: 1, html: "x" }, { title: "x", html: " \n" },
      { title: "x", html: "x\0" }, { title: "x\0", html: "x" }, { title: "", html: "x" }, { title: " \n ", html: "x" },
      { title: "x\ny", html: "x" }, { title: "x\u007fy", html: "x" }, { title: "x\u0085y", html: "x" }, { title: "🟢".repeat(121), html: "x" },
      { title: "x".repeat(121), html: "x" },
      { title: "x", html: "é".repeat(MAX_HTML_BYTES / 2 + 1) }]) {
      assert.throws(() => parseHtmlDocument(value));
    }
  });

  it("only recognizes validated html_document metadata", () => {
    assert.deepEqual(htmlDocumentFromMetadata({ kind: "html_document", title: "Counter", html: "<p>0</p>" }), { title: "Counter", html: "<p>0</p>" });
    for (const value of [undefined, { kind: "delegation", html: "x" }, { kind: "html_document", title: "x", html: "" }]) {
      assert.equal(htmlDocumentFromMetadata(value), null);
    }
  });

  it("requires a dedicated and closed zest-html fence", () => {
    assert.deepEqual(htmlDocumentFromFence("```zest-html Counter\n<button>+</button>\n```"), { title: "Counter", html: "<button>+</button>" });
    assert.deepEqual(htmlDocumentFromFence("~~~~zest-html\r\n<p>ok</p>\r\n~~~~~\n"), { title: "HTML preview", html: "<p>ok</p>" });
    for (const text of ["```html\nx\n```", "```ZEST-HTML\nx\n```", "```zest-html\nx", "```zest-html\nx\n```unfinished",
      "````zest-html\nx\n```", "```zest-html\nx\n~~~", "before\n```zest-html\nx\n```", "```zest-html\nx\n```\nafter",
      "```zest-html\n\n```", "    ```zest-html\nx\n```", "```zest-html\nx\n```\ny\n```"] ) {
      assert.equal(htmlDocumentFromFence(text), null, text);
    }
  });

  for (const fence of ["```", "~~~", "````", "~~~~"]) {
    for (const newline of ["\n", "\r\n"]) {
      it(`preserves raw ${fence} HTML through the Markdown pipeline with ${JSON.stringify(newline)} line endings`, () => {
        const html = [
          "  ",
          '<pre id="paths"></pre>',
          "<script>",
          'const forward = "C:/example/chart.png";',
          String.raw`const backward = "C:\\example\\chart.png";`,
          'const url = "file:///C:/example/chart.png";',
          'const markdown = "![chart](C:/example/chart.png)";',
          'document.getElementById("paths").textContent = [forward, backward, url, markdown].join(" | ");',
          "</script>",
          "\t ",
          "",
        ].join(newline);
        const fenced = `${fence}zest-html Paths${newline}${html}${newline}${fence}`;
        const expected = { title: "Paths", html };
        assert.deepEqual(htmlDocumentFromFence(fenced), expected);
        const blocks = splitRenderableBlocks(hoistLocalImages(`Before${newline}${newline}${fenced}${newline}${newline}After`), false);
        assert.equal(blocks.length, 3);
        const document = htmlDocumentFromFence(blocks[1].text);
        assert.deepEqual(document, expected);
        assert.ok(document);
        assert.ok(isolatedHtml(document).endsWith(html));
      });
    }
  }

  it("puts policy and RTC bootstrap before untrusted bytes", () => {
    const source = "<meta http-equiv='Content-Security-Policy' content='default-src *'><script>evil()</script>";
    const prepared = isolatedHtml({ title: '"><script>title()</script>', html: source });
    assert.ok(prepared.indexOf("RTCPeerConnection") < prepared.indexOf(source));
    assert.ok(prepared.includes("&lt;script&gt;title()&lt;/script&gt;"));
    assert.ok(HTML_CSP.includes("sandbox allow-scripts"));
    assert.ok(HTML_CSP.includes("connect-src 'none'"));
    assert.ok(HTML_CSP.includes("worker-src 'none'"));
    assert.ok(prepared.endsWith(source));
  });

  it("exports an HTML filename without path or reserved-name semantics", () => {
    assert.equal(htmlFilename("../CON"), "..-CON.html");
    assert.equal(htmlFilename("CON"), "_CON.html");
    assert.equal(htmlFilename(""), "document.html");
    assert.equal(htmlFilename("Counter"), "Counter.html");
  });
});
