import { ImageIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import type { WallpaperFilterId, WallpaperView } from "@/lib/types";
import { cn } from "@/lib/utils";

/**
 * The looks the add-on can render, in the order they are offered. Mirrors
 * `WALLPAPER_FILTERS` in `zest-plugin-api`; the host rejects anything else.
 */
const WALLPAPER_FILTERS: {
  id: WallpaperFilterId;
  label: string;
  hint: string;
}[] = [
  { id: "none", label: "Original", hint: "Your photo, unchanged." },
  {
    id: "print",
    label: "Print",
    hint: "A dotted print texture. Your photo keeps its color.",
  },
  {
    id: "frosted",
    label: "Frosted",
    hint: "A soft blur, like frosted glass. Easiest to read text over.",
  },
  {
    id: "noir",
    label: "Noir",
    hint: "Black and white, with a little grain.",
  },
];

type Props = {
  value: WallpaperView | null;
  className?: string;
  busy?: boolean;
  onChoose?: () => void;
  onClear?: () => void;
  onFilterChange?: (filter: WallpaperFilterId) => void;
};

export function WallpaperCard({
  value,
  className,
  busy = false,
  onChoose,
  onClear,
  onFilterChange,
}: Props) {
  if (!value) {
    return (
      <div className={cn("mt-2 text-[11px] text-muted-foreground", className)}>
        Checking…
      </div>
    );
  }

  const hasImage = value.status === "ready" && Boolean(value.imageDataUrl);
  const selected = WALLPAPER_FILTERS.find((filter) => filter.id === value.filter);
  const picking = busy || !hasImage || !onFilterChange;

  return (
    <div
      className={cn(
        "mt-2 overflow-hidden rounded-lg border border-border/70 bg-secondary/30",
        className
      )}
    >
      <div className="flex min-w-0 items-center gap-3 px-2.5 py-2.5">
        <div className="grid size-12 shrink-0 place-items-center overflow-hidden rounded-md bg-background ring-1 ring-border/70">
          {hasImage ? (
            <img
              src={value.imageDataUrl ?? ""}
              alt=""
              className={cn(
                "size-full object-cover",
                value.filter === "print" && "image-pixelated"
              )}
            />
          ) : (
            <ImageIcon className="size-5 text-muted-foreground" aria-hidden="true" />
          )}
        </div>
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-semibold">
            {hasImage ? value.sourceName || "Wallpaper" : "No image yet"}
          </div>
          <div className="mt-0.5 text-[11px] text-muted-foreground">
            {hasImage ? (selected?.label ?? "Original") : value.detail}
          </div>
        </div>
      </div>
      <div className="border-t border-border/60" />
      <div className="flex flex-wrap items-center gap-1.5 px-2.5 py-2">
        <Button
          type="button"
          size="sm"
          variant="secondary"
          disabled={busy || !onChoose}
          onClick={() => onChoose?.()}
        >
          {hasImage ? "Change image" : "Choose image"}
        </Button>
        {hasImage ? (
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={busy || !onClear}
            onClick={() => onClear?.()}
          >
            Remove
          </Button>
        ) : null}
      </div>
      {/*
       * Real radios rather than a row of toggle buttons: exactly one look
       * applies, and native inputs bring arrow-key movement and the "3 of 4"
       * announcement that a hand-rolled button group would have to reimplement.
       */}
      <fieldset
        className="m-0 flex flex-wrap items-center gap-1.5 border-0 border-t border-border/60 px-2.5 py-2"
        disabled={picking}
      >
        <legend className="sr-only">Background look</legend>
        {WALLPAPER_FILTERS.map((filter) => (
          <label key={filter.id} className="contents">
            <input
              type="radio"
              name="wallpaper-filter"
              className="peer sr-only"
              value={filter.id}
              checked={filter.id === value.filter}
              disabled={picking}
              onChange={() => onFilterChange?.(filter.id)}
            />
            <span
              className={cn(
                "cursor-pointer rounded-md border border-border/70 px-2 py-1 text-[11px] font-medium text-muted-foreground transition-colors",
                "hover:bg-secondary/60 hover:text-foreground",
                "peer-checked:border-primary peer-checked:bg-primary peer-checked:text-primary-foreground",
                "peer-focus-visible:outline-2 peer-focus-visible:outline-offset-2 peer-focus-visible:outline-ring",
                "peer-disabled:cursor-not-allowed peer-disabled:opacity-50"
              )}
            >
              {filter.label}
            </span>
          </label>
        ))}
      </fieldset>
      <p className="m-0 border-t border-border/60 px-2.5 py-2 text-[10px] leading-relaxed text-muted-foreground">
        {selected?.hint ?? WALLPAPER_FILTERS[0].hint}
      </p>
    </div>
  );
}
