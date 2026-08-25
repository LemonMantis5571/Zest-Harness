import { useEffect, useRef, useState } from "react";
import { CheckIcon, CopyIcon } from "lucide-react";

import {
  highlightCode,
  languageLabel,
  normalizeLang,
  releaseHighlight,
} from "@/lib/highlight";
import { ignoreExpectedFailure } from "@/lib/backgroundFailure";
import { cn } from "@/lib/utils";
import { markTrustedHtml } from "@/lib/safeHtml";

/** How long a code block must stop changing before it is worth highlighting. */
const HIGHLIGHT_SETTLE_MS = 120;

/** Per-mount id for the highlight queue. Module-scoped so it never collides. */
let nextKey = 1;

type Props = {
  code: string;
  language?: string | null;
  className?: string;
  /** Show language chip in the header (default true). */
  showLang?: boolean;
};

/**
 * Editor-style fenced code: language chip, copy, Shiki highlight.
 * Falls back to plain mono while highlighting or if Shiki fails.
 */
export function CodeBlock({
  code,
  language,
  className,
  showLang = true,
}: Props) {
  const lang = normalizeLang(language);
  const label = languageLabel(lang);
  const [html, setHtml] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  // Identifies this block to the highlight queue for the life of the component,
  // so its earlier requests get superseded, never another block's.
  const keyRef = useRef(`code-${nextKey++}`);

  useEffect(() => {
    let cancelled = false;
    // Two layers, both earning their place. The debounce stops a *streaming*
    // block from asking at all while it grows; the queue key means that when it
    // does ask, an older request for this same block is superseded rather than
    // raced. The work runs in a worker, so this does not cost main-thread time.
    const timer = window.setTimeout(() => {
      highlightCode(code, lang, keyRef.current)
        .then((next) => {
          if (!cancelled) setHtml(next);
        })
        .catch((error) => {
          // Superseded, released, or no worker available. The plain-text
          // fallback below is already correct, so leave what is on screen
          // rather than flashing it away.
          ignoreExpectedFailure(error, "highlight code block");
        });
    }, HIGHLIGHT_SETTLE_MS);

    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [code, lang]);

  // Free the slot when the block unmounts, so scrolling away does not leave
  // entries accumulating in the queue.
  useEffect(() => {
    const key = keyRef.current;
    return () => releaseHighlight(key);
  }, []);

  async function copy() {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1400);
    } catch {
      /* ignore */
    }
  }

  return (
    <div
      className={cn(
        "group/code my-3 overflow-hidden rounded-xl border border-border/70 bg-[#0d1117] last:mb-0",
        className
      )}
    >
      <div className="flex items-center justify-between gap-2 border-b border-border/50 px-3 py-1.5">
        {showLang ? (
          <span className="font-mono text-[11px] uppercase tracking-wide text-muted-foreground">
            {label}
          </span>
        ) : (
          <span />
        )}
        <button
          type="button"
          onClick={() => void copy()}
          title={copied ? "Copied" : "Copy"}
          className={cn(
            "inline-flex size-7 cursor-pointer items-center justify-center rounded-md text-muted-foreground outline-none transition-colors",
            "hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/40"
          )}
        >
          {copied ? (
            <CheckIcon className="size-3.5 text-primary" />
          ) : (
            <CopyIcon className="size-3.5" />
          )}
        </button>
      </div>
      <div className="overflow-x-auto">
        {html ? (
          <div
            className="code-highlight [&_pre]:m-0 [&_pre]:bg-transparent! [&_pre]:p-3 [&_pre]:text-[12.5px] [&_pre]:leading-[1.65] [&_code]:font-mono [&_code]:text-[12.5px] [&_span]:text-[length:inherit]"
            dangerouslySetInnerHTML={{ __html: markTrustedHtml(html, "shiki") }}
          />
        ) : (
          <pre className="m-0 p-3 font-mono text-[12.5px] leading-[1.65] text-muted-foreground whitespace-pre">
            {code}
          </pre>
        )}
      </div>
    </div>
  );
}

type DiffPreviewProps = {
  diff: string;
  path?: string;
  className?: string;
};

/** Live edit preview for write approvals with colored +/- lines. */
export function DiffPreview({
  diff,
  path,
  className,
  maxHeightClass = "max-h-56",
}: DiffPreviewProps & { maxHeightClass?: string }) {
  const lines = diff.split("\n");

  return (
    <div
      className={cn(
        "overflow-hidden border-b border-border/60 bg-[#0d1117]",
        className
      )}
    >
      {path ? (
        <div className="border-b border-border/40 px-3 py-1.5 font-mono text-[11px] text-muted-foreground">
          {path}
        </div>
      ) : null}
      <pre
        className={cn(
          "overflow-auto p-0 font-mono text-[11.5px] leading-[1.6]",
          maxHeightClass
        )}
      >
        {lines.map((line, i) => {
          const kind =
            line.startsWith("+") && !line.startsWith("+++")
              ? "add"
              : line.startsWith("-") && !line.startsWith("---")
                ? "del"
                : line.startsWith("@@")
                  ? "hunk"
                  : "ctx";
          return (
            <div
              key={`${i}:${line.slice(0, 24)}`}
              className={cn(
                "px-3 whitespace-pre-wrap break-all",
                kind === "add" && "bg-[rgba(39,166,68,0.12)] text-[#3fb950]",
                kind === "del" && "bg-[rgba(229,72,77,0.12)] text-[#f85149]",
                kind === "hunk" && "bg-[rgba(94,106,210,0.12)] text-[#828fff]",
                kind === "ctx" && "text-muted-foreground"
              )}
            >
              {line || " "}
            </div>
          );
        })}
      </pre>
    </div>
  );
}
