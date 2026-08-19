import { useState } from "react";
import { CheckIcon, RotateCcwIcon, SparklesIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  AVAILABLE_FONTS,
  DEFAULT_FONT_ID,
  applyFont,
  getSavedFontId,
  type AppFont,
  type FontCategory,
} from "@/lib/fonts";
import { cn } from "@/lib/utils";

type Props = {
  className?: string;
  onFontSelect?: (font: AppFont) => void;
};

const CATEGORY_LABELS: Record<FontCategory, string> = {
  sans: "Sans",
  serif: "Serif",
  mono: "Mono",
  variable: "Variable",
};

export function FontPicker({ className, onFontSelect }: Props) {
  const [selectedId, setSelectedId] = useState<string>(() => getSavedFontId());
  const [previewText, setPreviewText] = useState(
    "Zest: coding harness & orchestrator 0123456789"
  );

  function handleSelect(fontId: string) {
    setSelectedId(fontId);
    const applied = applyFont(fontId);
    onFontSelect?.(applied);
  }

  function handleReset() {
    handleSelect(DEFAULT_FONT_ID);
  }

  const activeFont = AVAILABLE_FONTS.find((f) => f.id === selectedId) ?? AVAILABLE_FONTS[0];

  return (
    <div className={cn("space-y-3", className)}>
      <div className="flex items-center justify-between">
        <div>
          <span className="text-xs font-medium text-foreground">Interface Typography</span>
          <p className="text-[11px] text-muted-foreground">
            Choose the primary typeface used across the Zest desktop shell and chats.
          </p>
        </div>
        {selectedId !== DEFAULT_FONT_ID && (
          <Button
            type="button"
            size="sm"
            variant="ghost"
            className="h-7 gap-1.5 px-2 text-xs text-muted-foreground hover:text-foreground"
            onClick={handleReset}
            title="Reset font to default (Geist)"
          >
            <RotateCcwIcon className="size-3" />
            Reset default
          </Button>
        )}
      </div>

      {/* Font Cards Grid */}
      <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
        {AVAILABLE_FONTS.map((font) => {
          const isSelected = font.id === selectedId;
          const isArizona = font.id === "abc-arizona";

          return (
            <button
              key={font.id}
              type="button"
              onClick={() => handleSelect(font.id)}
              className={cn(
                "group relative flex flex-col justify-between rounded-lg border p-2.5 text-left transition-all outline-none",
                isSelected
                  ? "border-primary/80 bg-primary/10 ring-1 ring-primary/40 shadow-xs"
                  : "border-border/70 bg-card/60 hover:border-border hover:bg-card/90"
              )}
            >
              <div className="flex w-full items-start justify-between gap-2">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-1.5">
                    <span
                      className="truncate text-xs font-semibold text-foreground"
                      style={{ fontFamily: font.fontFamily }}
                    >
                      {font.name}
                    </span>
                    {isArizona && (
                      <span className="inline-flex items-center gap-0.5 rounded-full bg-amber-500/15 px-1.5 py-0.2 text-[9px] font-medium text-amber-400">
                        <SparklesIcon className="size-2.5" />
                        Featured
                      </span>
                    )}
                  </div>
                  <p className="mt-0.5 line-clamp-2 text-[10.5px] leading-tight text-muted-foreground">
                    {font.description}
                  </p>
                </div>

                <div className="flex items-center gap-1">
                  <span className="rounded bg-secondary/80 px-1.5 py-0.5 text-[9px] font-medium tracking-wide text-muted-foreground uppercase">
                    {CATEGORY_LABELS[font.category]}
                  </span>
                  {isSelected ? (
                    <div className="flex size-4 shrink-0 items-center justify-center rounded-full bg-primary text-primary-foreground">
                      <CheckIcon className="size-2.5 stroke-[3]" />
                    </div>
                  ) : null}
                </div>
              </div>

              {/* Mini Sample Preview Line */}
              <div
                className="mt-2.5 rounded border border-border/40 bg-background/60 px-2 py-1 text-[11px] text-foreground/90 transition-colors group-hover:border-border/70"
                style={{ fontFamily: font.fontFamily }}
              >
                Aa Bb Gg 123 &amp; {`{ } =>`}
              </div>
            </button>
          );
        })}
      </div>

      {/* Live Sample Area */}
      <div className="rounded-lg border border-border/80 bg-card/40 p-3">
        <div className="flex items-center justify-between text-[11px] text-muted-foreground">
          <span>Live Preview ({activeFont.name})</span>
          <span className="font-mono text-[10px] text-muted-foreground/70">{activeFont.category}</span>
        </div>
        <div
          className="mt-1.5 text-sm text-foreground transition-all"
          style={{ fontFamily: activeFont.fontFamily }}
        >
          <input
            type="text"
            value={previewText}
            onChange={(e) => setPreviewText(e.target.value)}
            className="w-full rounded border border-border/60 bg-background/80 px-2.5 py-1.5 text-sm text-foreground outline-none focus-visible:ring-1 focus-visible:ring-ring/50"
            placeholder="Type anything to test the font..."
          />
        </div>
      </div>
    </div>
  );
}
