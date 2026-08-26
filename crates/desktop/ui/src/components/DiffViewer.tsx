import {
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import {
  ChevronDownIcon,
  GitBranchIcon,
  XIcon,
} from "lucide-react";

import { DiffPreview } from "@/components/CodeBlock";
import { Button } from "@/components/ui/button";
import { generateReadingDiff, type ReadingDiffView } from "@/lib/api";
import { ignoreExpectedFailure } from "@/lib/backgroundFailure";
import { splitDiffSections, type DiffSection } from "@/lib/diffSections";
import { makeReadingDiff, type ReadingDiff } from "@/lib/readingDiff";
import { cn } from "@/lib/utils";

export type DiffViewerTarget = {
  path: string;
  diff: string;
  source?: "tool" | "branch";
  changeId?: string;
};

type Props = {
  target: DiffViewerTarget | null;
  onClose: () => void;
  branch?: string | null;
  baseBranch?: string | null;
  width?: number;
  onResize?: (width: number) => void;
  storageKey?: string;
};

type DiffView = "reading" | "full";

function stripDiffMetadata(diff: string): string {
  return diff
    .split("\n")
    .filter(
      (line) =>
        !/^(?:diff --git |index |--- (?:a\/|b\/|\/dev\/null)|\+\+\+ (?:a\/|b\/|\/dev\/null)|old mode |new mode |similarity |rename from |rename to |copy from |copy to )/.test(
          line
        )
    )
    .join("\n");
}

function sectionKey(section: DiffSection, index: number): string {
  return `${section.path}:${index}`;
}

/** Changed-file review sidebar rendered in place for desktop WebView compatibility. */
export function DiffViewer({
  target,
  onClose,
  branch,
  baseBranch,
  width = 520,
  onResize,
  storageKey,
}: Props) {
  const titleId = useId();
  const [view, setView] = useState<DiffView>(() => {
    if (!storageKey || typeof window === "undefined") return "reading";
    return window.localStorage.getItem(storageKey) === "full" ? "full" : "reading";
  });
  const [reading, setReading] = useState<ReadingDiff | ReadingDiffView | null>(null);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const resizeCleanup = useRef<(() => void) | null>(null);

  useEffect(() => {
    if (!target) return;
    setCollapsed(new Set());
    const fallback = makeReadingDiff(target.diff);
    setReading(fallback);
    let cancelled = false;
    void generateReadingDiff(target.diff)
      .then((result) => {
        if (!cancelled) setReading(result);
      })
      .catch((error) => {
        // The local conservative view remains useful when the provider is
        // unavailable or returns an invalid plan.
        ignoreExpectedFailure(error, "generate reading diff");
      });
    return () => {
      cancelled = true;
    };
  }, [target]);

  useLayoutEffect(() => {
    if (!storageKey || typeof window === "undefined") return;
    const saved = window.localStorage.getItem(storageKey);
    setView(saved === "full" ? "full" : "reading");
  }, [storageKey]);

  useEffect(() => {
    if (!storageKey || typeof window === "undefined") return;
    window.localStorage.setItem(storageKey, view);
  }, [storageKey, view]);

  useEffect(() => () => resizeCleanup.current?.(), []);

  function clampWidth(value: number): number {
    const maximum = typeof window === "undefined"
      ? 760
      : Math.min(760, Math.max(360, window.innerWidth - 320));
    return Math.max(360, Math.min(maximum, value));
  }

  function beginResize(event: ReactPointerEvent<HTMLButtonElement>) {
    if (!onResize) return;
    event.preventDefault();
    const originX = event.clientX;
    const originWidth = width;
    const move = (next: PointerEvent) => onResize(clampWidth(originWidth + originX - next.clientX));
    const end = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", end);
      resizeCleanup.current = null;
    };
    resizeCleanup.current?.();
    resizeCleanup.current = end;
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end, { once: true });
  }

  function handleResizeKey(event: ReactKeyboardEvent<HTMLButtonElement>) {
    if (!onResize) return;
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      event.preventDefault();
      onResize(clampWidth(width + (event.key === "ArrowLeft" ? 24 : -24)));
    } else if (event.key === "Home") {
      event.preventDefault();
      onResize(360);
    } else if (event.key === "End") {
      event.preventDefault();
      onResize(clampWidth(760));
    }
  }

  useEffect(() => {
    if (!target) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [target, onClose]);

  const activeDiff =
    target && view === "reading" && reading ? reading.diff : target?.diff ?? "";
  const sections = useMemo(
    () => splitDiffSections(activeDiff, target?.path ?? ""),
    [activeDiff, target?.path]
  );
  const hiddenCount = reading && "hiddenImports" in reading ? reading.hiddenImports : 0;
  const foldedCount =
    reading && "foldedContextLines" in reading
      ? reading.foldedContextLines
      : reading?.foldedLines ?? 0;
  const totalAdded = sections.reduce((sum, section) => sum + section.added, 0);
  const totalRemoved = sections.reduce((sum, section) => sum + section.removed, 0);
  const hasBranchContext = target?.source === "branch" || Boolean(branch || baseBranch);

  function toggleSection(key: string) {
    setCollapsed((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }

  if (!target) return null;

  return (
    <aside
        role="dialog"
        aria-labelledby={titleId}
        className="relative z-40 flex h-full min-w-0 shrink-0 flex-col overflow-hidden border-l border-border/80 bg-background text-foreground shadow-2xl outline-none animate-in slide-in-from-right-2 duration-150 max-md:absolute max-md:inset-0 max-md:!w-full"
        style={{ width }}
      >
        {onResize ? (
          <button
            type="button"
            role="separator"
            aria-label="Resize branch changes pane"
            aria-orientation="vertical"
            aria-valuemin={360}
            aria-valuemax={760}
            aria-valuenow={Math.round(width)}
            className="absolute -left-1 top-0 z-50 h-full w-2 cursor-col-resize touch-none bg-transparent outline-none after:absolute after:inset-y-0 after:left-[3px] after:w-px after:bg-border/70 hover:after:bg-primary focus-visible:after:bg-primary"
            onPointerDown={beginResize}
            onKeyDown={handleResizeKey}
          />
        ) : null}
        <header className="shrink-0 border-b border-border/70 bg-card">
          <div className="flex items-start justify-between gap-3 px-3 py-2.5">
            <div className="min-w-0">
              <div id={titleId} className="flex items-center gap-1.5 text-xs font-medium">
                <GitBranchIcon className="size-3.5 text-primary/80" aria-hidden="true" />
                <span>{hasBranchContext ? "Branch changes" : "File changes"}</span>
                <ChevronDownIcon className="size-3 text-muted-foreground/70" aria-hidden="true" />
              </div>
              <div
                className="mt-1 truncate font-mono text-[10px] text-muted-foreground"
                title={hasBranchContext ? `${baseBranch ?? "base"} → ${branch ?? "current"}` : target.path}
              >
                {hasBranchContext
                  ? `${baseBranch ?? "base"} → ${branch ?? "current"}`
                  : `${sections.length} ${sections.length === 1 ? "file" : "files"} changed`}
              </div>
            </div>
            <div className="flex shrink-0 items-center gap-1">
              <div className="flex items-center rounded-md border border-border/70 bg-background/30 p-0.5">
                <button
                  type="button"
                  className={cn(
                    "rounded px-2 py-1 text-[10px] transition-colors",
                    view === "reading"
                      ? "bg-secondary text-foreground"
                      : "text-muted-foreground hover:text-foreground"
                  )}
                  onClick={() => setView("reading")}
                >
                  Clean
                </button>
                <button
                  type="button"
                  className={cn(
                    "rounded px-2 py-1 text-[10px] transition-colors",
                    view === "full"
                      ? "bg-secondary text-foreground"
                      : "text-muted-foreground hover:text-foreground"
                  )}
                  onClick={() => setView("full")}
                >
                  Raw
                </button>
              </div>
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                title="Close changes"
                aria-label="Close changes"
                onClick={onClose}
              >
                <XIcon />
              </Button>
            </div>
          </div>
          <div className="flex items-center gap-2 border-t border-border/50 px-3 py-1.5 text-[11px]">
            <span className="text-muted-foreground">
              {sections.length} {sections.length === 1 ? "file" : "files"}
            </span>
            <span className="text-primary">+{totalAdded}</span>
            <span className="text-destructive">−{totalRemoved}</span>
          </div>
          {view === "reading" && reading && (hiddenCount > 0 || foldedCount > 0) ? (
            <div className="border-t border-border/50 px-3 py-1.5 text-[10px] text-muted-foreground/75">
              Clean view
              {hiddenCount > 0 ? ` · ${hiddenCount} import lines hidden` : null}
              {foldedCount > 0 ? ` · ${foldedCount} lines folded` : null}
            </div>
          ) : null}
        </header>

        <div className="min-h-0 flex-1 overflow-y-auto bg-background">
          {sections.length > 0 ? (
            sections.map((section, index) => {
              const key = sectionKey(section, index);
              const isCollapsed = collapsed.has(key);
              const displayDiff = view === "reading" ? stripDiffMetadata(section.diff) : section.diff;
              return (
                <section key={key} className="border-b border-border/60 last:border-b-0">
                  <button
                    type="button"
                    aria-expanded={!isCollapsed}
                    className="flex w-full min-w-0 items-center gap-1.5 px-3 py-2 text-left transition-colors hover:bg-secondary/35 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/40"
                    onClick={() => toggleSection(key)}
                  >
                    <ChevronDownIcon
                      className={cn(
                        "size-3 shrink-0 text-muted-foreground/70 transition-transform duration-150",
                        isCollapsed && "-rotate-90"
                      )}
                      aria-hidden="true"
                    />
                    <span className="min-w-0 flex-1 truncate font-mono text-[11px] font-medium text-foreground/90" title={section.path}>
                      {section.path}
                    </span>
                    <span className="shrink-0 font-mono text-[10px] text-primary">+{section.added}</span>
                    <span className="shrink-0 font-mono text-[10px] text-destructive">−{section.removed}</span>
                  </button>
                  {!isCollapsed ? (
                    <DiffPreview
                      diff={displayDiff}
                      className="border-b-0 rounded-none bg-background"
                      maxHeightClass="max-h-none"
                    />
                  ) : null}
                </section>
              );
            })
          ) : (
            <div className="px-3 py-8 text-center text-xs text-muted-foreground">
              No changes to show.
            </div>
          )}
        </div>
      </aside>
  );
}
