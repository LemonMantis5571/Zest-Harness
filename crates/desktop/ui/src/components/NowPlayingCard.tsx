import { useEffect, useState } from "react";
import {
  Music2Icon,
  PauseIcon,
  PlayIcon,
  SkipBackIcon,
  SkipForwardIcon,
  Volume2Icon,
  VolumeXIcon,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Slider } from "@/components/ui/slider";
import type { NowPlayingView } from "@/lib/types";
import { cn } from "@/lib/utils";

type MediaAction = "previous" | "toggle" | "next";

type Props = {
  value: NowPlayingView | null;
  className?: string;
  onControl?: (action: MediaAction) => void;
  onVolumeChange?: (volumePercent: number) => void;
  controlBusy?: boolean;
};

export function NowPlayingCard({
  value,
  className,
  onControl,
  onVolumeChange,
  controlBusy = false,
}: Props) {
  const [volumeDraft, setVolumeDraft] = useState(value?.volumePercent ?? 0);

  useEffect(() => {
    if (value?.volumePercent != null) setVolumeDraft(value.volumePercent);
  }, [value?.volumePercent]);

  if (!value) {
    return (
      <div className={cn("mt-2 text-[11px] text-muted-foreground", className)}>
        Checking…
      </div>
    );
  }
  if (!value.title) {
    const emptyMessage =
      value.status === "disabled"
        ? "Turn it on in Settings."
        : value.status === "unavailable"
          ? "Music is not available right now."
          : "No music playing.";
    return (
      <div
        className={cn(
          "mt-2 rounded-md border border-dashed border-border/70 px-2.5 py-2 text-[11px] text-muted-foreground",
          className
        )}
      >
        {emptyMessage}
      </div>
    );
  }

  const byline = value.artist || value.album;
  const progress =
    value.positionSecs != null && value.durationSecs != null && value.durationSecs > 0
      ? `${formatMediaTime(value.positionSecs)} / ${formatMediaTime(value.durationSecs)}`
      : null;
  const hasControls = Boolean(onControl || onVolumeChange);
  const isPlaying = value.status === "playing";
  const hasVolume = value.volumePercent != null;
  const canPrevious = value.canPrevious !== false;
  const canToggle = value.canToggle !== false;
  const canNext = value.canNext !== false;

  return (
    <div
      className={cn(
        "mt-2 overflow-hidden rounded-lg border border-border/70 bg-secondary/30",
        className
      )}
    >
      <div className="flex min-w-0 items-center gap-3 px-2.5 py-2.5">
        <div className="grid size-12 shrink-0 place-items-center overflow-hidden rounded-md bg-background ring-1 ring-border/70">
          {value.artworkDataUrl ? (
            <img src={value.artworkDataUrl} alt="" className="size-full object-cover" />
          ) : (
            <Music2Icon className="size-5 text-muted-foreground" aria-hidden="true" />
          )}
        </div>
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-semibold" title={value.title}>
            {value.title}
          </div>
          {byline ? (
            <div className="mt-0.5 truncate text-[11px] text-muted-foreground" title={byline}>
              {byline}
            </div>
          ) : null}
          <div className="mt-1 flex min-w-0 items-center gap-2 text-[10px] text-muted-foreground">
            <span className="truncate">{value.status === "paused" ? "Paused" : "Playing"}</span>
            {progress ? <span className="shrink-0 tabular-nums">{progress}</span> : null}
          </div>
        </div>
      </div>

      {hasControls ? (
        <>
          <div className="border-t border-border/60" />
          <div className="flex items-center justify-center gap-2 px-2.5 py-2">
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              title="Previous"
              aria-label="Previous"
              disabled={controlBusy || !onControl || !canPrevious}
              onClick={() => onControl?.("previous")}
            >
              <SkipBackIcon aria-hidden="true" />
            </Button>
            <Button
              type="button"
              variant="secondary"
              size="icon-sm"
              title={isPlaying ? "Pause" : "Play"}
              aria-label={isPlaying ? "Pause" : "Play"}
              disabled={controlBusy || !onControl || !canToggle}
              onClick={() => onControl?.("toggle")}
            >
              {isPlaying ? <PauseIcon aria-hidden="true" /> : <PlayIcon aria-hidden="true" />}
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              title="Next"
              aria-label="Next"
              disabled={controlBusy || !onControl || !canNext}
              onClick={() => onControl?.("next")}
            >
              <SkipForwardIcon aria-hidden="true" />
            </Button>
          </div>
          <div className="border-t border-border/60" />
          <div className="flex items-center gap-2 px-2.5 py-2">
            {volumeDraft <= 0 ? (
              <VolumeXIcon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
            ) : (
              <Volume2Icon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
            )}
            <Slider
              aria-label="Volume"
              value={volumeDraft}
              min={0}
              max={100}
              step={1}
              disabled={controlBusy || !onVolumeChange || !hasVolume}
              className="min-w-0 flex-1"
              onValueChange={(next) => {
                if (typeof next === "number") setVolumeDraft(next);
              }}
              onValueCommitted={(next) => {
                if (typeof next === "number") onVolumeChange?.(next);
              }}
            />
            <span className="w-8 shrink-0 text-right text-[10px] tabular-nums text-muted-foreground">
              {hasVolume ? `${Math.round(volumeDraft)}%` : "—"}
            </span>
          </div>
        </>
      ) : null}
    </div>
  );
}

function formatMediaTime(secs: number): string {
  const total = Math.max(0, Math.floor(secs));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}
