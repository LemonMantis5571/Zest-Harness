import { useLayoutEffect, useRef, useState } from "react";
import {
  ChevronRightIcon,
  FilePenLineIcon,
  FileTextIcon,
  GlobeIcon,
  ListIcon,
  Maximize2Icon,
  SearchIcon,
  SparklesIcon,
  TerminalIcon,
  XIcon,
  ZapIcon,
} from "lucide-react";

import { DiffPreview } from "@/components/CodeBlock";
import { Button } from "@/components/ui/button";
import {
  approvalTitle,
  isEmptyArgsPreview,
  parseMcpToolName,
} from "@/lib/mcpDisplay";
import { displayToolSummary, wasInterrupted } from "@/lib/toolDisplay";
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
 * Compact tool row — quiet chrome, expands for detail.
 * Rows with a diff open the full DiffViewer on click.
 */
export function ToolCallRow({ tool, onResolveApproval, onOpenDiff, asCard }: Props) {
  const awaiting = tool.status === "awaiting_approval";
  const [busy, setBusy] = useState<ApprovalChoice | null>(null);
  const [open, setOpen] = useState(false);
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
  const showArgs = hasDiff;

  if (awaiting && !asCard) {
    return (
      <div
        data-tool-id={tool.id}
        className="flex min-h-8 w-full max-w-full items-center gap-2 rounded-lg px-2 py-1 text-left"
      >
        <span className="grid size-5 shrink-0 place-items-center rounded-md bg-amber-500/15 text-amber-400/90">
          {mcp ? (
            <ZapIcon className="size-3" />
          ) : isCommand || isDelegation ? (
            <TerminalIcon className="size-3" />
          ) : (
            <FilePenLineIcon className="size-3" />
          )}
        </span>
        <TruncateWithHover
          text={tool.path || tool.summary || tool.name}
          className="min-w-0 flex-1 font-mono text-[11.5px] text-muted-foreground/75"
        />
        <span className="shrink-0 text-[10px] text-amber-400/80">Awaiting approval</span>
      </div>
    );
  }

  if (awaiting) {
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

  const delegation =
    tool.metadata?.kind === "delegation" ? tool.metadata : null;
  const label = delegation ? "Delegate" : toolLabel(tool.name);
  const target = toolTarget(tool, delegation);
  const summaryText = displayToolSummary(tool.summary);
  const hasBody =
    (Boolean(summaryText) && summaryText !== (tool.path?.trim() ?? "")) || hasDiff;
  const canOpenDiff = hasDiff && Boolean(onOpenDiff) && !mcp;

  return (
    <div
      className={cn(
        "group/tool-row w-full max-w-full rounded-md",
        tool.status === "error" && "bg-destructive/5"
      )}
    >
      <button
        type="button"
        disabled={!hasBody}
        onClick={() => {
          if (canOpenDiff) {
            openDiff();
            return;
          }
          if (hasBody) setOpen((v) => !v);
        }}
        className={cn(
          "flex min-h-7 w-full items-center gap-2 rounded-md px-1 py-1 text-left outline-none transition-colors",
          "hover:bg-accent/70 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/40",
          hasBody ? "cursor-pointer" : "cursor-default"
        )}
      >
        <span className="relative flex size-4 shrink-0 items-center justify-center text-muted-foreground/80">
          <span
            className={cn(
              "transition-opacity duration-100",
              hasBody && "group-hover/tool-row:opacity-0",
              open && "opacity-0"
            )}
          >
            <ToolActionIcon tool={tool} />
          </span>
          {hasBody ? (
            <ChevronRightIcon
              className={cn(
                "absolute size-3 transition-[opacity,transform] duration-150 group-hover/tool-row:opacity-100",
                open ? "rotate-90 opacity-100" : "opacity-0"
              )}
            />
          ) : null}
        </span>
        <span
          className={cn(
            "shrink-0 text-xs font-medium text-foreground/90",
            tool.status === "running" && "shimmer-text text-foreground/80"
          )}
        >
          {label}
        </span>
        <TruncateWithHover
          text={target}
          className="min-w-0 flex-1 rounded-sm bg-muted/70 px-1.5 py-0.5 font-mono text-[11.5px] text-muted-foreground transition-colors group-hover/tool-row:bg-accent group-hover/tool-row:text-foreground"
        />
        {canOpenDiff ? (
          <Maximize2Icon className="size-3 shrink-0 text-muted-foreground/50" />
        ) : hasBody ? (
          <ChevronRightIcon
            className={cn(
              "size-3 shrink-0 text-muted-foreground/50 transition-transform duration-150",
              open && "rotate-90"
            )}
          />
        ) : null}
      </button>
      {open && !canOpenDiff ? (
        <div className="mt-0.5 mb-1 ml-2 flex flex-col gap-1 border-l border-border/60 py-0.5 pl-3.5 pr-2">
          {summaryText ? (
            <pre className="max-h-48 overflow-auto whitespace-pre-wrap font-mono text-[11px] leading-relaxed text-muted-foreground/90">
              {summaryText}
            </pre>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function toolLabel(name: string): string {
  const labels: Record<string, string> = {
    bash: "Run",
    delegate_external: "Delegate",
    edit_file: "Edit",
    glob: "Find",
    grep: "Search",
    list_dir: "List",
    read_file: "Read",
    web_search: "Search web",
    write_file: "Write",
  };
  const mcp = parseMcpToolName(name);
  if (mcp) return mcp.server;
  return labels[name] ?? name.replaceAll("_", " ");
}

function toolTarget(
  tool: ToolPart,
  delegation: Extract<
    NonNullable<ToolPart["metadata"]>,
    { kind: "delegation" }
  > | null
): string {
  if (delegation) return `${delegation.provider_id} · ${delegation.model}`;
  const mcp = parseMcpToolName(tool.name);
  if (mcp) {
    const path = tool.path?.trim() ?? "";
    const sep = " · ";
    const at = path.indexOf(sep);
    if (at >= 0) return path.slice(at + sep.length);
    return mcp.tool;
  }
  if (tool.path?.trim()) return tool.path;
  const summaryText = displayToolSummary(tool.summary);
  if (summaryText) return summaryText;
  if (wasInterrupted(tool.summary)) return "Interrupted";

  if (tool.status === "running") {
    const runningTargets: Record<string, string> = {
      bash: "Running command",
      delegate_external: "Working",
      edit_file: "Preparing edit",
      glob: "Searching files",
      grep: "Searching text",
      list_dir: "Listing files",
      read_file: "Reading file",
      web_search: "Searching web",
      write_file: "Preparing file",
    };
    return runningTargets[tool.name] ?? "Working";
  }

  return tool.status === "error" ? "Failed" : "Completed";
}

function ToolActionIcon({ tool }: { tool: ToolPart }) {
  if (tool.status === "error") {
    return <XIcon className="size-3 text-destructive" aria-hidden />;
  }

  const icon = (() => {
    switch (tool.name) {
      case "bash":
        return <TerminalIcon className="size-3.5" aria-hidden />;
      case "delegate_external":
        return <SparklesIcon className="size-3.5" aria-hidden />;
      case "edit_file":
      case "write_file":
        return <FilePenLineIcon className="size-3.5" aria-hidden />;
      case "glob":
      case "grep":
        return <SearchIcon className="size-3.5" aria-hidden />;
      case "list_dir":
        return <ListIcon className="size-3.5" aria-hidden />;
      case "web_search":
        return <GlobeIcon className="size-3.5" aria-hidden />;
      case "read_file":
        return <FileTextIcon className="size-3.5" aria-hidden />;
      default:
        if (parseMcpToolName(tool.name)) {
          return <ZapIcon className="size-3.5 text-emerald-400" aria-hidden />;
        }
        return <FileTextIcon className="size-3.5" aria-hidden />;
    }
  })();

  return (
    <span className={cn(tool.status === "running" && "animate-pulse")}>
      {icon}
    </span>
  );
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
