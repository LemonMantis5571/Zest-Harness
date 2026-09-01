import { useEffect, useId, useRef, useState } from "react";
import { Maximize2Icon, MinusIcon, PlusIcon, RotateCcwIcon, XIcon } from "lucide-react";
import { createPortal } from "react-dom";

import { Button } from "@/components/ui/button";
import { safeImageSrc } from "@/lib/imageSrc";

const DEFAULT_ZOOM = 1;
const MIN_ZOOM = 0.5;
const MAX_ZOOM = 4;
const ZOOM_STEP = 0.25;

function clampZoom(value: number) {
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, value));
}

type Props = {
  src?: string | null;
  alt?: string | null;
};

/**
 * Inline chat image that opens a zoom overlay on click.
 * Same overlay idea as Mermaid: click to expand, wheel or +/- to zoom.
 */
export function ZoomableImage({ src, alt }: Props) {
  const href = safeImageSrc(src);
  const [expanded, setExpanded] = useState(false);
  const [zoom, setZoom] = useState(DEFAULT_ZOOM);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  const label = alt?.trim() || "Image";

  useEffect(() => {
    if (!expanded) return;

    const previousOverflow = document.body.style.overflow;
    const trigger = triggerRef.current;
    document.body.style.overflow = "hidden";
    closeButtonRef.current?.focus();

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopImmediatePropagation();
        setExpanded(false);
        return;
      }
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
    document.addEventListener("keydown", onKeyDown, true);

    return () => {
      document.removeEventListener("keydown", onKeyDown, true);
      document.body.style.overflow = previousOverflow;
      trigger?.focus();
    };
  }, [expanded]);

  if (!href) return null;

  return (
    <>
      <div className="group/image relative my-3 max-w-full last:mb-0">
        <button
          ref={triggerRef}
          type="button"
          className="relative block max-w-full cursor-zoom-in overflow-hidden rounded-xl outline-none focus-visible:ring-2 focus-visible:ring-ring/60"
          title="Expand image"
          aria-label={`Expand ${label}`}
          onClick={() => {
            setZoom(DEFAULT_ZOOM);
            setExpanded(true);
          }}
        >
          <span className="pointer-events-none absolute right-3 top-3 z-10 inline-flex items-center gap-1.5 rounded-md border border-border/70 bg-card/90 px-2 py-1 text-[11px] text-muted-foreground opacity-0 shadow-sm transition-opacity group-hover/image:opacity-100 group-focus-within/image:opacity-100">
            <Maximize2Icon className="size-3.5" aria-hidden="true" />
            <span>Expand</span>
          </span>
          <img
            src={href}
            alt={alt ?? ""}
            referrerPolicy="no-referrer"
            className="max-h-[28rem] w-auto max-w-full rounded-xl"
          />
        </button>
      </div>

      {expanded
        ? createPortal(
            <div className="fixed inset-0 z-[90] flex items-center justify-center bg-black/80 p-2 sm:p-4">
              <button
                type="button"
                aria-label="Close image"
                className="absolute inset-0 cursor-pointer"
                onClick={() => setExpanded(false)}
              />
              <div
                role="dialog"
                aria-modal="true"
                aria-labelledby={titleId}
                className="relative z-10 flex h-full w-full flex-col overflow-hidden rounded-xl border border-border bg-card shadow-2xl animate-in zoom-in-95 fade-in duration-150"
              >
                <header className="flex shrink-0 items-center justify-between gap-3 border-b border-border/60 px-4 py-3">
                  <h2 id={titleId} className="truncate text-sm font-semibold">
                    {label}
                  </h2>
                  <Button
                    ref={closeButtonRef}
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    title="Close image"
                    aria-label="Close image"
                    onClick={() => setExpanded(false)}
                  >
                    <XIcon />
                  </Button>
                </header>
                <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border/60 bg-muted/20 px-4 py-2">
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
                    <img
                      src={href}
                      alt={alt ?? ""}
                      referrerPolicy="no-referrer"
                      className="max-w-none rounded-md"
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
