import { useEffect, useId, useRef, useState } from "react";
import { Maximize2Icon, MinusIcon, PlusIcon, RotateCcwIcon, XIcon } from "lucide-react";
import { createPortal } from "react-dom";

import { CodeBlock } from "@/components/CodeBlock";
import { Button } from "@/components/ui/button";
import { markTrustedHtml } from "@/lib/safeHtml";
import { cn } from "@/lib/utils";

type Props = {
  code: string;
  /** Keep incomplete fences readable while the assistant is streaming. */
  streaming?: boolean;
};

type RenderState =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ready"; svg: string }
  | { status: "error" };

let mermaidModule: Promise<typeof import("mermaid")> | null = null;
let nextDiagramId = 1;

const DEFAULT_ZOOM = 1;
const MIN_ZOOM = 0.5;
const MAX_ZOOM = 3;
const ZOOM_STEP = 0.25;

function clampZoom(value: number) {
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, value));
}

function loadMermaid() {
  return (mermaidModule ??= import("mermaid"));
}

/**
 * Render Mermaid lazily and only after a fenced block has settled.
 *
 * Mermaid's strict security mode keeps SVG links and labels from becoming an
 * HTML injection path. Invalid or still-growing diagrams keep the existing
 * copyable code-block fallback instead of breaking the whole assistant turn.
 */
export function MermaidBlock({ code, streaming = false }: Props) {
  const [state, setState] = useState<RenderState>({ status: "idle" });
  const [expanded, setExpanded] = useState(false);
  const [zoom, setZoom] = useState(DEFAULT_ZOOM);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const titleId = useId();

  useEffect(() => {
    let cancelled = false;

    if (streaming || !code.trim()) {
      setState({ status: "idle" });
      return () => {
        cancelled = true;
      };
    }

    setState({ status: "loading" });
    const id = `zest-mermaid-${nextDiagramId++}`;

    void loadMermaid()
      .then(({ default: mermaid }) => {
        mermaid.initialize({
          startOnLoad: false,
          securityLevel: "strict",
          theme: "dark",
          fontFamily: "inherit",
        });
        return mermaid.render(id, code);
      })
      .then(({ svg }) => {
        if (!cancelled) setState({ status: "ready", svg });
      })
      .catch(() => {
        if (!cancelled) setState({ status: "error" });
      });

    return () => {
      cancelled = true;
    };
  }, [code, streaming]);

  useEffect(() => {
    if (!expanded) return;

    const previousOverflow = document.body.style.overflow;
    const previouslyFocused = document.activeElement;
    const trigger = triggerRef.current;
    document.body.style.overflow = "hidden";
    closeButtonRef.current?.focus();

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setExpanded(false);
      if (event.key === "+" || event.key === "=") {
        event.preventDefault();
        setZoom((value) => clampZoom(value + ZOOM_STEP));
      }
      if (event.key === "-") {
        event.preventDefault();
        setZoom((value) => clampZoom(value - ZOOM_STEP));
      }
      if (event.key === "0") {
        event.preventDefault();
        setZoom(DEFAULT_ZOOM);
      }
    };
    document.addEventListener("keydown", onKeyDown);

    return () => {
      document.removeEventListener("keydown", onKeyDown);
      document.body.style.overflow = previousOverflow;
      if (trigger) {
        trigger.focus();
      } else if (previouslyFocused instanceof HTMLElement) {
        previouslyFocused.focus();
      }
    };
  }, [expanded]);

  if (state.status !== "ready") {
    return <CodeBlock code={code} language="mermaid" />;
  }

  return (
    <>
      <div className="group/diagram relative my-3 overflow-hidden rounded-xl border border-border/70 bg-[#0d1117] last:mb-0">
        <button
          ref={triggerRef}
          type="button"
          className="relative block w-full cursor-zoom-in overflow-x-auto p-3 text-left outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/60"
          title="Expand diagram"
          aria-label="Expand Mermaid diagram"
          onClick={() => {
            setZoom(DEFAULT_ZOOM);
            setExpanded(true);
          }}
        >
          <span className="pointer-events-none absolute right-3 top-3 z-10 inline-flex items-center gap-1.5 rounded-md border border-border/70 bg-[#0d1117]/90 px-2 py-1 text-[11px] text-muted-foreground opacity-0 shadow-sm transition-opacity group-hover/diagram:opacity-100 group-focus-within/diagram:opacity-100">
            <Maximize2Icon className="size-3.5" aria-hidden="true" />
            <span>Expand</span>
          </span>
          <RenderedDiagram svg={state.svg} className="justify-center [&_svg]:max-w-full" />
        </button>
      </div>

      {expanded
        ? createPortal(
            <div className="fixed inset-0 z-[90] flex items-center justify-center bg-black/75 p-2 sm:p-4">
              <button
                type="button"
                aria-label="Close diagram"
                className="absolute inset-0 cursor-pointer"
                onClick={() => setExpanded(false)}
              />
              <div
                role="dialog"
                aria-modal="true"
                aria-labelledby={titleId}
                className="relative z-10 flex h-full w-full flex-col overflow-hidden rounded-xl border border-border bg-[#0d1117] shadow-2xl animate-in zoom-in-95 fade-in duration-150"
              >
                <header className="flex shrink-0 items-center justify-between gap-3 border-b border-border/60 px-4 py-3">
                  <div className="min-w-0">
                    <h2 id={titleId} className="text-sm font-semibold">
                      Mermaid diagram
                    </h2>
                    <p className="text-[11px] text-muted-foreground">
                      Scroll or zoom your window to inspect the full chart
                    </p>
                  </div>
                  <Button
                    ref={closeButtonRef}
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    title="Close diagram"
                    aria-label="Close diagram"
                    onClick={() => setExpanded(false)}
                  >
                    <XIcon />
                  </Button>
                </header>
                <div
                  data-zoom-controls="true"
                  className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border/60 bg-muted/20 px-4 py-2"
                >
                  <span className="mr-1 text-xs font-medium text-muted-foreground">Zoom</span>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    title="Zoom out"
                    aria-label="Zoom out"
                    disabled={zoom <= MIN_ZOOM}
                    onClick={() => setZoom((value) => clampZoom(value - ZOOM_STEP))}
                  >
                    <MinusIcon />
                    <span>Zoom out</span>
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="min-w-16 tabular-nums"
                    title="Reset zoom"
                    aria-label={`Reset zoom to 100 percent (currently ${Math.round(zoom * 100)} percent)`}
                    onClick={() => setZoom(DEFAULT_ZOOM)}
                  >
                    <RotateCcwIcon aria-hidden="true" />
                    <span>{Math.round(zoom * 100)}%</span>
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    title="Zoom in"
                    aria-label="Zoom in"
                    disabled={zoom >= MAX_ZOOM}
                    onClick={() => setZoom((value) => clampZoom(value + ZOOM_STEP))}
                  >
                    <PlusIcon />
                    <span>Zoom in</span>
                  </Button>
                  <span className="ml-auto text-[11px] text-muted-foreground">
                    Ctrl/Cmd + wheel or + / -
                  </span>
                </div>
                <div
                  className="min-h-0 flex-1 overflow-auto p-4 sm:p-8"
                  onWheel={(event) => {
                    if (!event.ctrlKey && !event.metaKey) return;
                    event.preventDefault();
                    setZoom((value) => clampZoom(value - event.deltaY * 0.01));
                  }}
                >
                  <div
                    className="flex min-h-full w-max min-w-full items-center justify-center"
                    style={{ zoom }}
                  >
                    <RenderedDiagram
                      svg={state.svg}
                      className="shrink-0 [&_svg]:block [&_svg]:max-w-none"
                    />
                  </div>
                </div>
              </div>
            </div>,
            document.body
          )
        : null}
    </>
  );
}

function RenderedDiagram({ svg, className }: { svg: string; className?: string }) {
  return (
    <span
      className={cn("mermaid-diagram flex", className)}
      dangerouslySetInnerHTML={{ __html: markTrustedHtml(svg, "mermaid") }}
    />
  );
}
