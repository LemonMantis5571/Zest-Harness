/**
 * Shiki setup and the actual highlight call.
 *
 * Split out from `highlight.ts` so the **worker** can import it without
 * dragging in anything main-thread. Nothing here touches the DOM.
 *
 * Uses createHighlighterCore + the JS regex engine with statically imported
 * langs/themes so the desktop webview does not need WASM or runtime chunk fetches
 * (those fail silently and left us on the plain mono fallback).
 */
import { createHighlighterCore, type HighlighterCore } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";

import themeGithubDark from "@shikijs/themes/github-dark-default";

import langBash from "@shikijs/langs/bash";
import langC from "@shikijs/langs/c";
import langCpp from "@shikijs/langs/cpp";
import langCss from "@shikijs/langs/css";
import langCsharp from "@shikijs/langs/csharp";
import langDiff from "@shikijs/langs/diff";
import langGo from "@shikijs/langs/go";
import langHtml from "@shikijs/langs/html";
import langJava from "@shikijs/langs/java";
import langJavascript from "@shikijs/langs/javascript";
import langJson from "@shikijs/langs/json";
import langJsx from "@shikijs/langs/jsx";
import langMarkdown from "@shikijs/langs/markdown";
import langPowershell from "@shikijs/langs/powershell";
import langPython from "@shikijs/langs/python";
import langRust from "@shikijs/langs/rust";
import langScss from "@shikijs/langs/scss";
import langShellscript from "@shikijs/langs/shellscript";
import langSql from "@shikijs/langs/sql";
import langToml from "@shikijs/langs/toml";
import langTsx from "@shikijs/langs/tsx";
import langTypescript from "@shikijs/langs/typescript";
import langYaml from "@shikijs/langs/yaml";

import { normalizeLang } from "./codeLanguage.ts";

export { languageLabel, normalizeLang } from "./codeLanguage.ts";

const THEME = "github-dark-default";

let highlighterPromise: Promise<HighlighterCore> | null = null;

function getHighlighter(): Promise<HighlighterCore> {
  if (!highlighterPromise) {
    highlighterPromise = createHighlighterCore({
      themes: [themeGithubDark],
      langs: [
        langTypescript,
        langTsx,
        langJavascript,
        langJsx,
        langPython,
        langRust,
        langGo,
        langJava,
        langC,
        langCpp,
        langCsharp,
        langJson,
        langToml,
        langYaml,
        langMarkdown,
        langHtml,
        langCss,
        langScss,
        langSql,
        langBash,
        langShellscript,
        langPowershell,
        langDiff,
      ],
      engine: createJavaScriptRegexEngine(),
    }).catch((err) => {
      // Allow a later CodeBlock mount to retry after a transient failure.
      highlighterPromise = null;
      throw err;
    });
  }
  return highlighterPromise;
}

function escapeHtml(code: string): string {
  return code
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

/** Highlight to HTML (Shiki wraps in pre/code; host styles the chrome). */
export async function highlightToHtml(
  code: string,
  langHint?: string | null
): Promise<string> {
  const lang = normalizeLang(langHint);
  // No grammar package for plain text — keep structure consistent with Shiki.
  if (lang === "plaintext") {
    return `<pre class="shiki ${THEME}" tabindex="0"><code>${escapeHtml(code)}</code></pre>`;
  }
  const highlighter = await getHighlighter();
  return highlighter.codeToHtml(code, {
    lang,
    theme: THEME,
  });
}
