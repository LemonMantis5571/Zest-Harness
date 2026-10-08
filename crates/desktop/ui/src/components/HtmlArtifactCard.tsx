import { useEffect, useRef, useState } from "react";
import { Code2Icon, DownloadIcon, Maximize2Icon, RotateCcwIcon, XIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import { getBackend } from "@/lib/backend";
import { ignoreExpectedFailure } from "@/lib/backgroundFailure";
import type { HtmlDocument, PreparedHtmlView } from "@/lib/htmlArtifacts";
import { cn } from "@/lib/utils";

type View = { kind: "closed" } | { kind: "preparing" } | { kind: "open"; prepared: PreparedHtmlView };

export function HtmlArtifactCard({ title, html, previewNotRun = false }: HtmlDocument & { previewNotRun?: boolean }) {
  const backend = getBackend();
  const [view, setView] = useState<View>({ kind: "closed" });
  const [source, setSource] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState("");
  const generation = useRef(0);
  const prepared = useRef<PreparedHtmlView | null>(null);
  const container = useRef<HTMLElement>(null);

  // A late prepare must be released after unmount, reset, or document replacement.
  useEffect(() => {
    setView({ kind: "closed" });
    setExpanded(false);
    setNotice("");
    return () => {
      generation.current += 1;
      const previous = prepared.current;
      prepared.current = null;
      if (previous) void backend.releaseHtmlView(previous.token).catch((error) => ignoreExpectedFailure(error, "release expiring HTML view"));
    };
  }, [backend, title, html]);

  useEffect(() => {
    if (!expanded) return;
    const previous = document.activeElement;
    container.current?.focus();
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.stopPropagation();
        setExpanded(false);
      }
      if (event.key !== "Tab") return;
      const buttons = container.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)");
      const first = buttons?.[0];
      const last = buttons?.[buttons.length - 1];
      if (event.shiftKey && (document.activeElement === first || document.activeElement === container.current)) {
        event.preventDefault();
        last?.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first?.focus();
      }
    }
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("keydown", onKey, true);
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  }, [expanded]);

  async function open() {
    if (view.kind === "preparing") return;
    const attempt = ++generation.current;
    const previous = prepared.current;
    prepared.current = null;
    setView({ kind: "preparing" });
    setNotice("");
    try {
      if (previous) await backend.releaseHtmlView(previous.token);
      const next = await backend.prepareHtmlView({ title, html });
      if (attempt !== generation.current) {
        await backend.releaseHtmlView(next.token);
        return;
      }
      prepared.current = next;
      setView({ kind: "open", prepared: next });
    } catch (error) {
      if (attempt !== generation.current) return;
      setView({ kind: "closed" });
      setNotice(error instanceof Error ? error.message : String(error));
    }
  }

  async function save() {
    if (saving) return;
    setSaving(true);
    setNotice("");
    try {
      if (await backend.saveHtmlDocument({ title, html })) setNotice("HTML saved.");
    } catch {
      setNotice("Could not save HTML. Try again.");
    } finally {
      setSaving(false);
    }
  }

  return (
    <section
      ref={container}
      tabIndex={expanded ? -1 : undefined}
      role={expanded ? "dialog" : "region"}
      aria-modal={expanded || undefined}
      aria-label={title || "HTML preview"}
      data-slot="html-artifact"
      data-expanded={expanded}
      className={cn("my-3 overflow-hidden rounded-xl border border-border bg-card shadow-sm", expanded && "fixed inset-4 z-[100] m-0 flex flex-col shadow-2xl")}
    >
      <div className="flex flex-wrap items-center gap-2 border-b border-border px-3 py-2">
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-medium">{title || "HTML preview"}</div>
          <div className="text-xs text-muted-foreground">{previewNotRun && view.kind === "closed" ? "Preview not run" : view.kind === "open" ? "Running offline" : "HTML document"} · Offline only</div>
        </div>
        <Button variant="ghost" size="sm" aria-pressed={source} onClick={() => setSource((value) => !value)}><Code2Icon aria-hidden />Source</Button>
        <Button variant="ghost" size="sm" disabled={view.kind !== "open"} onClick={() => void open()}><RotateCcwIcon aria-hidden />Reset</Button>
        <Button variant="ghost" size="sm" aria-expanded={expanded} onClick={() => setExpanded((value) => !value)}>
          {expanded ? <XIcon aria-hidden /> : <Maximize2Icon aria-hidden />}{expanded ? "Close expanded preview" : "Expand"}
        </Button>
        <Button variant="ghost" size="sm" disabled={saving} onClick={() => void save()}><DownloadIcon aria-hidden />{saving ? "Saving…" : "Save HTML"}</Button>
      </div>
      <p className="px-3 pt-2 text-xs text-muted-foreground">Saved HTML opens outside Zest's sandbox.</p>
      <div className={cn("min-h-0", expanded && "flex-1 overflow-auto")}>
        {source ? <pre data-slot="html-source" className="max-h-96 overflow-auto whitespace-pre-wrap break-words p-3 font-mono text-xs">{html}</pre> : null}
        {view.kind === "open" ? (
          <iframe
            title={`${title || "HTML preview"} interactive preview`}
            src={view.prepared.url}
            sandbox="allow-scripts"
            referrerPolicy="no-referrer"
            className={cn("block w-full border-0 bg-white", expanded ? "h-full min-h-96" : "h-80")}
          />
        ) : (
          <div className="flex flex-col items-center gap-3 px-4 py-8 text-center">
            <p className="max-w-sm text-sm text-muted-foreground">Open to run this document's scripts in an offline sandbox.</p>
            <Button disabled={view.kind === "preparing"} onClick={() => void open()}>{view.kind === "preparing" ? "Opening…" : "Open"}</Button>
          </div>
        )}
      </div>
      {notice ? <p role="status" className="border-t border-border px-3 py-2 text-xs text-muted-foreground">{notice}</p> : null}
    </section>
  );
}
