import type { Components } from "react-markdown";
import { isValidElement, memo, useMemo, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { ArrowUpRightIcon, Globe2Icon } from "lucide-react";

import { CodeBlock } from "@/components/CodeBlock";
import { MermaidBlock } from "@/components/MermaidBlock";
import { ZoomableImage } from "@/components/ZoomableImage";
import { safeHttpUrl } from "@/lib/externalLinks";
import { linkClassName } from "@/lib/linkify";
import { splitBlocks } from "@/lib/markdownBlocks";
import { cn } from "@/lib/utils";

function codeText(children: ReactNode): string {
  if (typeof children === "string") return children;
  if (Array.isArray(children)) return children.map(codeText).join("");
  if (children == null || typeof children === "boolean") return "";
  if (typeof children === "object" && "props" in children) {
    const nested = (children as { props?: { children?: ReactNode } }).props
      ?.children;
    return codeText(nested);
  }
  return String(children);
}

type LinkElement = {
  props?: { href?: string; children?: ReactNode };
};

function standaloneLink(children: ReactNode): { href: string; label: ReactNode } | null {
  const child = Array.isArray(children) && children.length === 1 ? children[0] : children;
  if (!isValidElement(child)) return null;
  const props = (child as unknown as LinkElement).props;
  const href = safeHttpUrl(props?.href);
  if (!href) return null;
  return { href, label: props?.children ?? href };
}

function LinkPreview({ href, label }: { href: string; label: ReactNode }) {
  let host = href;
  try {
    host = new URL(href).hostname.replace(/^www\./, "");
  } catch {
    // Keep the raw URL when a compatible renderer gives us an unusual URL.
  }

  return (
    <div className="mb-3 flex max-w-[500px] items-center gap-3 rounded-xl border border-border/80 bg-card/45 px-3.5 py-3 shadow-[0_1px_2px_rgb(0_0_0/12%)] last:mb-0">
      <div className="grid size-8 shrink-0 place-items-center rounded-lg bg-secondary text-muted-foreground">
        <Globe2Icon className="size-4" aria-hidden />
      </div>
      <div className="min-w-0 flex-1">
        <div className="truncate text-[13px] font-semibold text-foreground">{label}</div>
        <div className="mt-0.5 truncate text-[11px] text-muted-foreground">{host} · Opened in Browser</div>
      </div>
      <a
        href={href}
        target="_blank"
        rel="noreferrer"
        className="inline-flex shrink-0 items-center gap-1 rounded-md bg-secondary px-2.5 py-1.5 text-xs font-semibold text-foreground transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
      >
        Open
        <ArrowUpRightIcon className="size-3" aria-hidden />
      </a>
    </div>
  );
}

function componentsFor(streaming: boolean): Components {
  return {
  p: ({ children }) => {
    const link = standaloneLink(children);
    return link ? (
      <LinkPreview href={link.href} label={link.label} />
    ) : (
      <p className="mb-3 last:mb-0 leading-[1.65]">{children}</p>
    );
  },
  strong: ({ children }) => (
    <strong className="font-semibold text-foreground">{children}</strong>
  ),
  em: ({ children }) => <em className="italic">{children}</em>,
  a: ({ href, children }) => {
    const safeHref = safeHttpUrl(href);
    return safeHref ? (
      <a href={safeHref} target="_blank" rel="noreferrer" className={linkClassName}>
        {children}
      </a>
    ) : (
      <span>{children}</span>
    );
  },
  ul: ({ children }) => (
    <ul className="mb-3 list-disc space-y-1 pl-5 last:mb-0">{children}</ul>
  ),
  ol: ({ children }) => (
    <ol className="mb-3 list-decimal space-y-1 pl-5 last:mb-0">{children}</ol>
  ),
  li: ({ children }) => <li className="leading-[1.65]">{children}</li>,
  h1: ({ children }) => (
    <h1 className="mb-2 mt-4 text-base font-semibold tracking-[-0.2px] first:mt-0">
      {children}
    </h1>
  ),
  h2: ({ children }) => (
    <h2 className="mb-2 mt-4 text-[15px] font-semibold tracking-[-0.2px] first:mt-0">
      {children}
    </h2>
  ),
  h3: ({ children }) => (
    <h3 className="mb-1.5 mt-3 text-sm font-semibold first:mt-0">{children}</h3>
  ),
  blockquote: ({ children }) => (
    <blockquote className="mb-3 border-l-2 border-border pl-3 text-muted-foreground last:mb-0">
      {children}
    </blockquote>
  ),
  code: ({ className, children }) => {
    const isBlock = Boolean(className?.includes("language-"));
    if (isBlock) {
      // Fenced blocks are handled by `pre` → CodeBlock.
      return <code className={className}>{children}</code>;
    }
    return (
      <code className="rounded-md bg-muted px-1 py-0.5 font-mono text-[12px] text-foreground">
        {children}
      </code>
    );
  },
  pre: ({ children }) => {
    const child = Array.isArray(children) ? children[0] : children;
    const props =
      child && typeof child === "object" && "props" in child
        ? (child as {
            props?: { className?: string; children?: ReactNode };
          }).props
        : undefined;
    const className = props?.className ?? "";
    const langMatch = /language-([\w+#.-]+)/.exec(className);
    const language = langMatch?.[1] ?? "plaintext";
    const code = codeText(props?.children ?? children).replace(/\n$/, "");
    return language.toLowerCase() === "mermaid" ? (
      <MermaidBlock code={code} streaming={streaming} />
    ) : (
      <CodeBlock code={code} language={language} streaming={streaming} />
    );
  },
  hr: () => <hr className="my-4 border-border/70" />,
  img: ({ src, alt }) => <ZoomableImage src={src} alt={alt} />,
  table: ({ children }) => (
    <div className="mb-4 overflow-x-auto rounded-xl border border-border/80 bg-card/30 shadow-[0_1px_2px_rgb(0_0_0/10%)] last:mb-0">
      <table className="min-w-full border-separate border-spacing-0 text-left text-[13px]">
        {children}
      </table>
    </div>
  ),
  thead: ({ children }) => <thead className="bg-secondary/80">{children}</thead>,
  tbody: ({ children }) => <tbody className="divide-y divide-border/60">{children}</tbody>,
  tr: ({ children }) => <tr className="transition-colors hover:bg-accent/25">{children}</tr>,
  th: ({ children }) => (
    <th className="border-b border-border px-3 py-2 text-[11px] font-semibold uppercase tracking-[0.04em] text-muted-foreground first:rounded-tl-xl last:rounded-tr-xl">
      {children}
    </th>
  ),
  td: ({ children }) => (
    <td className="px-3 py-2.5 align-top text-muted-foreground first:text-foreground">
      {children}
    </td>
  ),
  };
}

type Props = {
  children: string;
  className?: string;
  streaming?: boolean;
};

/**
 * One top-level markdown block.
 *
 * Memoized separately from its neighbours: while a message streams, only its
 * trailing block changes, so every settled block above skips re-parsing
 * entirely. That is the difference between O(n²) and O(n) over a long answer.
 */
const Block = memo(function Block({ text, streaming }: { text: string; streaming: boolean }) {
  const components = useMemo(() => componentsFor(streaming), [streaming]);

  return (
    <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
      {text}
    </ReactMarkdown>
  );
});

/**
 * GFM markdown for assistant (and muted thinking) bodies.
 *
 * Memoized twice over, and both levels are load-bearing rather than
 * micro-optimisations:
 *
 * - **This component** skips messages the reducer did not touch. Without it one
 *   streaming message re-parses every *finished* message in the transcript on
 *   every frame, so a long chat degrades as it grows.
 * - **Each block** skips the settled part of the message being streamed. A
 *   single growing string means re-parsing the whole document per frame; blocks
 *   mean re-parsing only the tail.
 */
export const Markdown = memo(function Markdown({
  children,
  className,
  streaming = false,
}: Props) {
  const blocks = useMemo(() => splitBlocks(children), [children]);

  return (
    <div
      className={cn(
        "max-w-none text-[15px] text-foreground wrap-break-word",
        className
      )}
    >
      {blocks.map((block) => (
        <Block key={block.key} text={block.text} streaming={streaming} />
      ))}
    </div>
  );
});
