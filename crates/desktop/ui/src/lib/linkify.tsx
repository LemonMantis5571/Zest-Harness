import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

/** Link colour follows `--link` so it stays readable on both palettes. */
export const linkClassName =
  "cursor-pointer text-link underline-offset-2 hover:underline";

/** Match http(s) URLs; trim trailing punctuation commonly stuck to pasted links. */
const URL_RE = /(https?:\/\/[^\s<>"']+)/gi;

function trimUrl(raw: string): { href: string; trailing: string } {
  let href = raw;
  let trailing = "";
  while (/[.,;:!?)\]}'"]$/.test(href)) {
    trailing = href.slice(-1) + trailing;
    href = href.slice(0, -1);
  }
  return { href, trailing };
}

/** Turn bare URLs in plain text into blue links (user bubbles). */
export function LinkifyText({
  text,
  className,
}: {
  text: string;
  className?: string;
}) {
  const nodes: ReactNode[] = [];
  let last = 0;
  let match: RegExpExecArray | null;
  const re = new RegExp(URL_RE.source, URL_RE.flags);
  let i = 0;
  while ((match = re.exec(text)) !== null) {
    if (match.index > last) {
      nodes.push(text.slice(last, match.index));
    }
    const { href, trailing } = trimUrl(match[0]);
    nodes.push(
      <a
        key={`u-${i++}`}
        href={href}
        target="_blank"
        rel="noreferrer"
        className={cn(linkClassName, className)}
        onClick={(e) => e.stopPropagation()}
      >
        {href}
      </a>
    );
    if (trailing) nodes.push(trailing);
    last = match.index + match[0].length;
  }
  if (last < text.length) nodes.push(text.slice(last));
  return <>{nodes}</>;
}
