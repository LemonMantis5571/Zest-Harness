import { useLayoutEffect, useRef, useState } from "react";
import {
  FilePenLineIcon,
  FileTextIcon,
  GlobeIcon,
  LightbulbIcon,
  ListIcon,
  Maximize2Icon,
  SearchIcon,
  SparklesIcon,
  TerminalIcon,
  WrenchIcon,
  XIcon,
  ZapIcon,
} from "lucide-react";

import { DiffPreview } from "@/components/CodeBlock";
import { HtmlArtifactCard } from "@/components/HtmlArtifactCard";
import { TraceHoverTitle } from "@/components/TraceHoverTitle";
import { Button } from "@/components/ui/button";
import { CodeBlock, CodeBlockContent } from "@/components/ui/code-block";
import {
  TraceCollapsible,
  TraceCollapsibleContent,
  TraceCollapsibleTrigger,
  TraceDescription,
  TraceIcon,
  TraceLine,
} from "@/components/ui/trace";
import { htmlDocumentFromMetadata } from "@/lib/htmlArtifacts";
import {
  approvalTitle,
  isEmptyArgsPreview,
  parseMcpToolName,
} from "@/lib/mcpDisplay";
import { displayToolSummary, wasInterrupted } from "@/lib/toolDisplay";
import { countDiffLines } from "@/lib/toolRuns";
import type { ApprovalChoice, ToolPart } from "@/lib/types";
import { cn } from "@/lib/utils";

type Props = {
  tool: ToolPart;
  onResolveApproval: (
    approvalId: string,
    decision: ApprovalChoice
  ) => Promise<void>;
  onOpenDiff?: (path: string, diff: string) => void;
  /**
   * Render the full approval card with its decision buttons.
   *
   * Off by default, which means the transcript shows a pending approval as a
   * one-line row and nothing else. The card lives in one fixed place above the
   * composer instead: as an inline element it moved with the scroll, drew a
   * fresh copy of Deny / Allow for session / Allow once per pending call, and
   * pushed the conversation off screen while offering no extra choice.
   */
  asCard?: boolean;
};

/**
 * A tool call: the approval card while it waits on a decision and `asCard` is
 * set, otherwise one line of the turn's `Trace`.
 */
export function ToolCallRow({ tool, onResolveApproval, onOpenDiff, asCard }: Props) {
  if (tool.status === "awaiting_approval" && asCard) {
    return (
      <ApprovalCard
        tool={tool}
        onResolveApproval={onResolveApproval}
        onOpenDiff={onOpenDiff}
      />
    );
  }
  return <ToolLine tool={tool} onOpenDiff={onOpenDiff} />;
}

function ApprovalCard({
  tool,
  onResolveApproval,
  onOpenDiff,
}: {
  tool: ToolPart;
  onResolveApproval: Props["onResolveApproval"];
  onOpenDiff?: Props["onOpenDiff"];
}) {
  const [busy, setBusy] = useState<ApprovalChoice | null>(null);
  const mcp = parseMcpToolName(tool.name);
  const hasDiff = Boolean(tool.diff?.trim()) && !isEmptyArgsPreview(tool.diff);

  async function resolve(decision: ApprovalChoice) {
    if (!tool.approvalId || busy !== null) return;
    setBusy(decision);
    try {
      await onResolveApproval(tool.approvalId, decision);
    } catch {
      setBusy(null);
    }
  }

  function openDiff() {
    if (!tool.diff?.trim() || !onOpenDiff) return;
    onOpenDiff(tool.path || tool.name, tool.diff);
  }

  // `bash` is the only tool that asks to run a command rather than change a
  // file; for it, `path` carries the command line verbatim.
  const isCommand = tool.name === "bash";
  const isDelegation = tool.name === "delegate_external";
  // Sends the conversation to a second provider; the card names which one.
  const isAdvisor = tool.name === "consult_advisor";
  const showArgs = hasDiff;
  const subtitle =
    (isDelegation && tool.summary) ||
    tool.path ||
    tool.summary ||
    (isCommand
      ? "Run a command"
      : isDelegation
        ? "Run an external worker"
        : mcp
          ? `Run ${mcp.tool} on the ${mcp.server} MCP server`
          : "Write to project file");
  return (
    <div data-tool-id={tool.id} className="w-full max-w-full overflow-visible">
      <div className="flex items-start gap-2.5 px-1 py-1">
        <div className="mt-0.5 grid size-6 place-items-center rounded-md bg-muted/80 text-foreground">
          {mcp ? (
            <ZapIcon className="size-3.5 text-emerald-400" />
          ) : isAdvisor ? (
            <LightbulbIcon className="size-3.5" />
          ) : isCommand || isDelegation ? (
            <TerminalIcon className="size-3.5" />
          ) : (
            <FilePenLineIcon className="size-3.5" />
          )}
        </div>
        <div className="min-w-0 flex-1">
          <div className="text-xs font-medium text-foreground">
            {approvalTitle(tool.name)}
          </div>
          <TruncateWithHover
            text={subtitle}
            side="below"
            className="mt-0.5 text-[11px] text-muted-foreground"
          />
        </div>
        {hasDiff && !mcp && onOpenDiff ? (
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            title="Open full diff"
            className="shrink-0"
            onClick={openDiff}
          >
            <Maximize2Icon className="size-3.5" />
          </Button>
        ) : null}
      </div>
      {showArgs && mcp && tool.diff ? (
        <pre className="mx-1 mb-1 max-h-40 overflow-auto rounded-md bg-muted/40 px-2.5 py-2 font-mono text-[11px] leading-relaxed text-muted-foreground">
          {tool.diff}
        </pre>
      ) : showArgs && tool.diff && !mcp ? (
        <button
          type="button"
          title="Open full diff"
          className="block w-full cursor-pointer text-left outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/40"
          onClick={openDiff}
        >
          <DiffPreview diff={tool.diff} />
        </button>
      ) : null}
      <div className="flex flex-wrap items-center justify-end gap-2 px-1 py-1.5">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={busy !== null}
          onClick={() => {
            void resolve("deny");
          }}
        >
          {busy === "deny" ? "Denying…" : "Deny"}
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={busy !== null}
          // The grant covers this tool and this exact target only, which is
          // what the row above is showing.
          title={
            isDelegation
              ? "Allow this external worker for the rest of the session"
              : isCommand
                ? "Stop asking about this exact command for the rest of the session"
                : `Stop asking about ${tool.path || "this file"} for the rest of the session`
          }
          onClick={() => {
            void resolve("session");
          }}
        >
          {busy === "session" ? "Allowing…" : "Allow for session"}
        </Button>
        <Button
          type="button"
          size="sm"
          disabled={busy !== null}
          onClick={() => {
            void resolve("once");
          }}
        >
          {busy === "once" ? (isCommand ? "Running…" : "Allowing…") : "Allow once"}
        </Button>
      </div>
    </div>
  );
}

/** Running / done / failed wording for each built-in tool. */
const VERBS: Record<string, [string, string, string]> = {
  bash: ["Running", "Ran", "Failed to run"],
  consult_advisor: ["Asking the advisor", "Asked the advisor", "Failed to ask the advisor"],
  edit_file: ["Editing", "Edited", "Failed to edit"],
  glob: ["Finding files", "Found files", "Failed to find files"],
  grep: ["Searching", "Searched", "Failed to search"],
  list_dir: ["Listing", "Listed", "Failed to list"],
  read_file: ["Reading", "Read", "Failed to read"],
  web_search: ["Searching the web", "Searched the web", "Failed to search the web"],
  write_file: ["Writing", "Wrote", "Failed to write"],
};

/** The verb a line opens with, in the tense its status calls for. */
function toolVerb(tool: ToolPart): string {
  if (tool.status === "awaiting_approval") return "Waiting for approval:";
  const mcp = parseMcpToolName(tool.name);
  const [running, done, failed] =
    VERBS[tool.name] ??
    (mcp
      ? [`Running ${mcp.tool} on`, `Ran ${mcp.tool} on`, `Failed ${mcp.tool} on`]
      : (() => {
          const label = tool.name.replaceAll("_", " ");
          return [`Running ${label}`, `Ran ${label}`, `Failed ${label}`];
        })());
  if (tool.status === "running") return running;
  if (tool.status === "error") return failed;
  return done;
}

/** What the call acted on: the path or command, else its result. */
function toolTarget(tool: ToolPart): string | undefined {
  const mcp = parseMcpToolName(tool.name);
  if (mcp) {
    const path = tool.path?.trim() ?? "";
    const sep = " · ";
    const at = path.indexOf(sep);
    return `${mcp.server}${at >= 0 ? `: ${path.slice(at + sep.length)}` : ""}`;
  }
  if (tool.path?.trim()) return tool.path.trim();
  const summary = displayToolSummary(tool.summary);
  if (summary) return summary;
  if (wasInterrupted(tool.summary)) return "(interrupted)";
  return undefined;
}

/** The line's title as one sentence, such as "Ran npm test". */
export function toolTitleText(tool: ToolPart): string {
  const verb = toolVerb(tool);
  const target = toolTarget(tool);
  return target ? `${verb} ${target}` : verb;
}

function lineStatus(tool: ToolPart): "running" | "error" | "pending" | undefined {
  if (tool.status === "running") return "running";
  if (tool.status === "error") return "error";
  if (tool.status === "awaiting_approval") return "pending";
  return undefined;
}

/**
 * One tool call as a `TraceLine`, as the AI Elements Tool pattern draws it.
 *
 * A call with a diff opens the full diff viewer on click; a call with output
 * expands to show it in a small code block; anything else is a plain line.
 * Must sit inside a `Trace`.
 */
export function ToolLine({
  tool,
  onOpenDiff,
}: {
  tool: ToolPart;
  onOpenDiff?: (path: string, diff: string) => void;
}) {
  const htmlDocument = tool.status === "done" ? htmlDocumentFromMetadata(tool.metadata) : null;
  if (htmlDocument) return <HtmlArtifactCard {...htmlDocument} />;

  const mcp = parseMcpToolName(tool.name);
  const hasDiff = Boolean(tool.diff?.trim()) && !isEmptyArgsPreview(tool.diff);
  const canOpenDiff = hasDiff && Boolean(onOpenDiff) && !mcp;
  const verb = toolVerb(tool);
  const target = toolTarget(tool);
  const summaryText = displayToolSummary(tool.summary);
  const hasBody =
    tool.status !== "awaiting_approval" &&
    Boolean(summaryText) &&
    summaryText !== (tool.path?.trim() ?? "");
  const counts = hasDiff && !mcp ? countDiffLines(tool.diff) : null;

  const line = (
    <TraceLine status={lineStatus(tool)} data-tool-id={tool.id}>
      <TraceIcon>
        <ToolActionIcon tool={tool} />
      </TraceIcon>
      <TraceHoverTitle text={toolTitleText(tool)}>
        {verb}
        {target ? <span className="font-mono text-[0.92em]"> {target}</span> : null}
      </TraceHoverTitle>
      {counts && (counts.added > 0 || counts.removed > 0) ? (
        <TraceDescription className="font-mono text-[11px]">
          {counts.added > 0 ? <span className="text-primary">+{counts.added}</span> : null}
          {counts.added > 0 && counts.removed > 0 ? " " : null}
          {counts.removed > 0 ? (
            <span className="text-destructive">-{counts.removed}</span>
          ) : null}
        </TraceDescription>
      ) : null}
      {canOpenDiff ? (
        <TraceIcon>
          <Maximize2Icon className="size-3" />
        </TraceIcon>
      ) : null}
    </TraceLine>
  );

  if (canOpenDiff) {
    return (
      <button
        type="button"
        title="Open full diff"
        onClick={() => onOpenDiff?.(tool.path || tool.name, tool.diff as string)}
      >
        {line}
      </button>
    );
  }
  if (!hasBody) return line;
  return (
    <TraceCollapsible>
      <TraceCollapsibleTrigger>{line}</TraceCollapsibleTrigger>
      <TraceCollapsibleContent>
        <CodeBlock size="sm" variant={tool.status === "error" ? "destructive" : "default"}>
          <CodeBlockContent wrap className="max-h-48">
            {summaryText}
          </CodeBlockContent>
        </CodeBlock>
      </TraceCollapsibleContent>
    </TraceCollapsible>
  );
}

function ToolActionIcon({ tool }: { tool: ToolPart }) {
  if (tool.status === "error") return <XIcon />;
  switch (tool.name) {
    case "bash":
      return <TerminalIcon />;
    case "delegate_external":
      return <SparklesIcon />;
    case "consult_advisor":
      return <LightbulbIcon />;
    case "edit_file":
    case "write_file":
      return <FilePenLineIcon />;
    case "glob":
    case "grep":
      return <SearchIcon />;
    case "list_dir":
      return <ListIcon />;
    case "web_search":
      return <GlobeIcon />;
    case "read_file":
      return <FileTextIcon />;
    default:
      if (parseMcpToolName(tool.name)) return <ZapIcon className="text-emerald-400" />;
      return <WrenchIcon />;
  }
}

/** Single-line truncate with in-tree hover card (no portal - WebView-safe). */
function TruncateWithHover({
  text,
  className,
  side = "above",
}: {
  text: string;
  className?: string;
  side?: "above" | "below";
}) {
  const textRef = useRef<HTMLSpanElement>(null);
  const [overflows, setOverflows] = useState(false);

  useLayoutEffect(() => {
    const el = textRef.current;
    if (!el) return;
    const measure = () => {
      setOverflows(el.scrollWidth > el.clientWidth + 1);
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [text]);

  return (
    <span className={cn("group/trunc relative min-w-0", className)}>
      <span ref={textRef} className="block truncate">
        {text}
      </span>
      {overflows ? (
        <span
          role="tooltip"
          className={cn(
            "pointer-events-none absolute left-0 z-50 hidden w-max max-w-[min(22rem,70vw)]",
            side === "below"
              ? "top-[calc(100%+6px)]"
              : "bottom-[calc(100%+6px)]",
            "rounded-md border border-border/80 bg-popover px-2.5 py-1.5 text-left text-[11px] leading-snug text-popover-foreground shadow-lg",
            "whitespace-pre-wrap break-words",
            "group-hover/trunc:block"
          )}
        >
          {text}
        </span>
      ) : null}
    </span>
  );
}
