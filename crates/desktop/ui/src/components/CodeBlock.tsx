import {
  CodeBlock as ReuiCodeBlock,
  CodeBlockCopyButton,
  CodeBlockHeader,
  CodeBlockLanguage,
  CodeBlockTitle,
} from "@/components/reui/code-block/code-block";
import { languageLabel, normalizeLang } from "@/lib/codeLanguage";
import { cn } from "@/lib/utils";

type Props = {
  code: string;
  language?: string | null;
  title?: string;
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
  title,
  className,
  showLang = true,
}: Props) {
  const lang = normalizeLang(language);
  const label = languageLabel(lang);
  const hasHeader = showLang || Boolean(title);

  return (
    <ReuiCodeBlock
      code={code}
      language={lang}
      label={`${label} code`}
      className={cn("my-3 overflow-hidden last:mb-0", className)}
    >
      {hasHeader ? (
        <CodeBlockHeader>
          {title ? <CodeBlockTitle>{title}</CodeBlockTitle> : null}
          {showLang ? <CodeBlockLanguage /> : null}
          <CodeBlockCopyButton
            value={code}
            position="inline"
            aria-label="Copy code"
            className="ml-auto text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/40"
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
        "overflow-hidden border-b border-border/60 bg-card",
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
                kind === "add" && "bg-primary/10 text-primary",
                kind === "del" && "bg-destructive/10 text-destructive",
                kind === "hunk" && "bg-primary/10 text-primary",
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
