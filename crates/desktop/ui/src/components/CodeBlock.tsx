import {
  CodeBlock as ReuiCodeBlock,
  CodeBlockCopyButton,
  CodeBlockHeader,
  CodeBlockLanguage,
} from "@/components/reui/code-block/code-block";
import { languageLabel, normalizeLang } from "@/lib/codeLanguage";
import { cn } from "@/lib/utils";

type Props = {
  code: string;
  language?: string | null;
  className?: string;
  /** Show language chip in the header (default true). */
  showLang?: boolean;
};

/**
 * Editor-style fenced code with the shared ReUI surface and copy affordance.
 * The public Zest wrapper stays small so Markdown and Mermaid keep one stable
 * rendering entry point while the richer surface owns highlighting and line UI.
 */
export function CodeBlock({
  code,
  language,
  className,
  showLang = true,
}: Props) {
  const lang = normalizeLang(language);
  const label = languageLabel(lang);

  return (
    <ReuiCodeBlock
      code={code}
      language={lang}
      label={`${label} code`}
      variant="ghost"
      className={cn(
        "group/code my-3 overflow-hidden rounded-xl border border-border/70 bg-[#0d1117] last:mb-0",
        "[&_pre]:px-3 [&_pre]:text-[12.5px] [&_pre]:leading-[1.65]",
        className
      )}
    >
      {showLang ? (
        <CodeBlockHeader className="border-border/50 px-3 py-1.5">
          <CodeBlockLanguage className="border-0 bg-transparent p-0 text-[11px] uppercase tracking-wide">
            {label}
          </CodeBlockLanguage>
          <CodeBlockCopyButton
            value={code}
            position="inline"
            alwaysVisible
            aria-label="Copy code"
            className="text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/40"
          />
        </CodeBlockHeader>
      ) : (
        <CodeBlockCopyButton
          value={code}
          position="pinned"
          alwaysVisible
          aria-label="Copy code"
          className="text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/40"
        />
      )}
    </ReuiCodeBlock>
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
