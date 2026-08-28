import { useState } from "react";
import { CheckIcon, RotateCcwIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  AVAILABLE_THEMES,
  DEFAULT_THEME_ID,
  applyTheme,
  getSavedThemeId,
  type AppTheme,
} from "@/lib/themes";
import { cn } from "@/lib/utils";

type Props = {
  className?: string;
  onThemeSelect?: (theme: AppTheme) => void;
};

function ThemePreview({ theme }: { theme: AppTheme }) {
  const { background, sidebar, card, primary } = theme.swatches;
  return (
    <div
      className="mt-2.5 h-14 overflow-hidden rounded-md border border-border/40"
      style={{ background }}
      aria-hidden
    >
      <div className="flex h-full">
        <div className="w-[22%] border-r border-foreground/10" style={{ background: sidebar }} />
        <div className="flex min-w-0 flex-1 flex-col gap-1 p-1.5">
          <div className="h-1.5 w-1/3 rounded-sm" style={{ background: primary }} />
          <div className="min-h-0 flex-1 rounded-sm" style={{ background: card }} />
        </div>
      </div>
    </div>
  );
}

export function ThemePicker({ className, onThemeSelect }: Props) {
  const [selectedId, setSelectedId] = useState<string>(() => getSavedThemeId());

  function handleSelect(themeId: string) {
    setSelectedId(themeId);
    const applied = applyTheme(themeId);
    onThemeSelect?.(applied);
  }

  function handleReset() {
    handleSelect(DEFAULT_THEME_ID);
  }

  return (
    <div className={cn("space-y-3", className)}>
      <div className="flex items-center justify-between gap-2">
        <div>
          <span className="text-xs font-medium text-foreground">Colour theme</span>
          <p className="text-[11px] text-muted-foreground">
            Palettes for the desktop shell, chats, and code.
          </p>
        </div>
        {selectedId !== DEFAULT_THEME_ID ? (
          <Button
            type="button"
            size="sm"
            variant="ghost"
            className="h-7 gap-1.5 px-2 text-xs text-muted-foreground hover:text-foreground"
            onClick={handleReset}
            title="Reset theme to default (Zest)"
          >
            <RotateCcwIcon className="size-3" />
            Reset default
          </Button>
        ) : null}
      </div>

      <div
        className="grid grid-cols-1 gap-2 sm:grid-cols-2"
        role="radiogroup"
        aria-label="Colour theme"
      >
        {AVAILABLE_THEMES.map((theme) => {
          const isSelected = theme.id === selectedId;
          return (
            <button
              key={theme.id}
              type="button"
              role="radio"
              aria-checked={isSelected}
              aria-label={`${theme.name}, ${theme.appearance} theme. ${theme.description}`}
              onClick={() => handleSelect(theme.id)}
              className={cn(
                "group relative flex flex-col rounded-lg border p-2.5 text-left transition-all outline-none focus-visible:ring-2 focus-visible:ring-ring/50",
                isSelected
                  ? "border-primary/80 bg-primary/10 ring-1 ring-primary/40 shadow-xs"
                  : "border-border/70 bg-card/60 hover:border-border hover:bg-card/90"
              )}
            >
              <div className="flex w-full items-start justify-between gap-2">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-1.5">
                    <span className="truncate text-xs font-semibold text-foreground">
                      {theme.name}
                    </span>
                    <span className="rounded bg-secondary/80 px-1.5 py-0.5 text-[9px] font-medium tracking-wide text-muted-foreground uppercase">
                      {theme.appearance}
                    </span>
                  </div>
                  <p className="mt-0.5 line-clamp-2 text-[10.5px] leading-tight text-muted-foreground">
                    {theme.description}
                  </p>
                </div>
                {isSelected ? (
                  <div className="flex size-4 shrink-0 items-center justify-center rounded-full bg-primary text-primary-foreground">
                    <CheckIcon className="size-2.5 stroke-[3]" />
                  </div>
                ) : null}
              </div>
              <ThemePreview theme={theme} />
            </button>
          );
        })}
      </div>
    </div>
  );
}
