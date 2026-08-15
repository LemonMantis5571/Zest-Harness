import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { FolderOpenIcon, Music2Icon, RefreshCwIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import { NowPlayingCard } from "@/components/NowPlayingCard";
import { TopbarPanel } from "@/components/TopbarPanel";
import { getBackend } from "@/lib/backend";
import { createNowPlayingCoordinator } from "@/lib/nowPlayingCoordinator";
import { nowPlayingButtonVisible, nowPlayingPluginState } from "@/lib/nowPlayingPluginState";
import type { NowPlayingView, PluginView } from "@/lib/types";

type MediaAction = "previous" | "toggle" | "next";

export function NowPlayingButton() {
  const [plugin, setPlugin] = useState<PluginView | null>(null);
  const [checked, setChecked] = useState(false);
  const [value, setValue] = useState<NowPlayingView | null>(null);
  const [loading, setLoading] = useState(false);
  const [folderBusy, setFolderBusy] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const coordinator = useMemo(() => createNowPlayingCoordinator<NowPlayingView>(), []);
  const refreshTimerRef = useRef<number | null>(null);
  const pluginRequestRef = useRef(0);

  const clearRefreshTimer = useCallback(() => {
    if (refreshTimerRef.current === null) return;
    window.clearTimeout(refreshTimerRef.current);
    refreshTimerRef.current = null;
  }, []);

  const readNowPlaying = useCallback(async () => {
    const result = await coordinator.read(() => getBackend().nowPlaying());
    if (result.status === "success") {
      if (result.committed) {
        setValue(result.value);
        setError(null);
      }
      return result.value;
    }
    if (result.status === "error" && result.committed) {
      setError(result.error instanceof Error ? result.error.message : "Could not read the music.");
    }
    return null;
  }, [coordinator]);

  const loadPlugin = useCallback(async () => {
    const requestId = ++pluginRequestRef.current;
    setLoading(true);
    setError(null);
    try {
      const next =
        (await getBackend().listPlugins()).find((candidate) => candidate.id === "now-playing") ??
        null;
      if (requestId !== pluginRequestRef.current) return;

      setPlugin(next);
      if (next?.enabled && next.available) {
        await readNowPlaying();
      } else {
        clearRefreshTimer();
        coordinator.invalidate();
        setValue(null);
      }
    } catch {
      if (requestId !== pluginRequestRef.current) return;
      clearRefreshTimer();
      coordinator.invalidate();
      setPlugin(null);
      setValue(null);
      setError("Could not load extras.");
    } finally {
      if (requestId === pluginRequestRef.current) {
        setChecked(true);
        setLoading(false);
      }
    }
  }, [clearRefreshTimer, coordinator, readNowPlaying]);

  useEffect(() => {
    void loadPlugin();
  }, [loadPlugin]);

  useEffect(() => {
    if (!plugin?.enabled || !plugin.available) return;
    const timer = window.setInterval(() => {
      void readNowPlaying();
    }, 5_000);
    return () => window.clearInterval(timer);
  }, [plugin?.available, plugin?.enabled, readNowPlaying]);

  useEffect(() => {
    if (!checked || plugin?.available) return;
    const timer = window.setInterval(() => {
      void loadPlugin();
    }, 5_000);
    return () => window.clearInterval(timer);
  }, [checked, loadPlugin, plugin?.available]);

  useEffect(() => {
    return () => {
      pluginRequestRef.current += 1;
      clearRefreshTimer();
      coordinator.dispose();
    };
  }, [clearRefreshTimer, coordinator]);

  async function togglePlugin() {
    if (!plugin || busy) return;
    const requestId = ++pluginRequestRef.current;
    clearRefreshTimer();
    coordinator.invalidate();
    setBusy(true);
    setError(null);
    try {
      const next = await getBackend().setPluginEnabled(plugin.id, !plugin.enabled);
      const row = next.find((candidate) => candidate.id === plugin.id) ?? plugin;
      setPlugin(row);
      if (row.enabled && row.available) {
        await readNowPlaying();
      } else {
        coordinator.invalidate();
        setValue(null);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not change this extra.");
    } finally {
      setBusy(false);
      if (requestId === pluginRequestRef.current) setLoading(false);
    }
  }

  async function controlNowPlaying(action: MediaAction) {
    if (!plugin?.enabled || !plugin.available || busy) return;
    clearRefreshTimer();
    setBusy(true);
    setError(null);
    const result = await coordinator.action(() => getBackend().controlNowPlaying(action));
    if (result.status === "success") {
      if (result.committed) {
        setValue(result.value);
        setError(null);
        refreshTimerRef.current = window.setTimeout(() => {
          refreshTimerRef.current = null;
          void readNowPlaying();
        }, 300);
      }
    } else if (result.status === "error" && result.committed) {
      setError(result.error instanceof Error ? result.error.message : "Could not change the music.");
    }
    setBusy(false);
  }

  async function changeVolume(volumePercent: number) {
    if (!plugin?.enabled || !plugin.available || busy) return;
    clearRefreshTimer();
    setBusy(true);
    setError(null);
    const result = await coordinator.action(() => getBackend().setNowPlayingVolume(volumePercent));
    if (result.status === "success") {
      if (result.committed) {
        setValue(result.value);
        setError(null);
      }
    } else if (result.status === "error" && result.committed) {
      setError(result.error instanceof Error ? result.error.message : "Could not change the volume.");
    }
    setBusy(false);
  }

  async function openPluginFolder() {
    setFolderBusy(true);
    setError(null);
    try {
      await getBackend().openPluginsFolder();
    } catch {
      setError("Could not open the add-on folder.");
    } finally {
      setFolderBusy(false);
    }
  }

  const handleOpenChange = useCallback(
    (nextOpen: boolean) => {
      if (nextOpen) void loadPlugin();
    },
    [loadPlugin]
  );

  const hasTrack = Boolean(plugin?.enabled && plugin.available && value?.title?.trim());
  const title = hasTrack ? value?.title?.trim() || "Music" : "Music";
  const artist = hasTrack ? value?.artist?.trim() : undefined;
  const trackLabel = artist ? `${title} · ${artist}` : title;
  const pluginState = nowPlayingPluginState(checked, plugin);
  // Every hook above still runs, so the poll that watches for a later install
  // keeps going and the button appears on its own once the add-on is there.
  if (!nowPlayingButtonVisible(pluginState)) return null;

  const pluginMessage =
    pluginState === "checking"
      ? "Checking for the add-on…"
      : pluginState === "missing"
        ? "Music add-on not found."
        : plugin?.detail || "Music add-on is not ready.";

  const recoveryActions = (
    <div className="flex flex-wrap gap-1.5">
      <Button
        type="button"
        size="sm"
        variant="secondary"
        disabled={folderBusy}
        onClick={() => void openPluginFolder()}
      >
        <FolderOpenIcon data-icon="inline-start" aria-hidden="true" />
        Open folder
      </Button>
      <Button
        type="button"
        size="sm"
        variant="outline"
        disabled={loading}
        onClick={() => void loadPlugin()}
      >
        <RefreshCwIcon data-icon="inline-start" aria-hidden="true" />
        Refresh
      </Button>
    </div>
  );

  return (
    <TopbarPanel
      icon={Music2Icon}
      label={trackLabel}
      triggerClassName="min-w-0 max-w-[230px] max-[420px]:size-7 max-[420px]:px-1.5"
      trigger={
        <span className="flex min-w-0 items-center gap-1.5">
          {value?.artworkDataUrl && hasTrack ? (
            <img
              src={value.artworkDataUrl}
              alt=""
              className="size-4 shrink-0 rounded-sm object-cover"
            />
          ) : (
            <Music2Icon data-icon="inline-start" aria-hidden="true" />
          )}
          <span className="min-w-0 truncate max-[420px]:hidden">{trackLabel}</span>
        </span>
      }
      onOpenChange={handleOpenChange}
    >
      <div className="flex flex-col gap-3">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <h2 className="m-0 text-sm font-semibold">Music</h2>
            <p className="m-0 mt-1 text-[11px] leading-relaxed text-muted-foreground">
              Optional add-on for music controls.
            </p>
          </div>
          {plugin?.available ? (
            <Button
              type="button"
              size="xs"
              variant={plugin.enabled ? "outline" : "default"}
              disabled={busy || loading}
              onClick={() => void togglePlugin()}
            >
              {busy ? "Wait…" : plugin.enabled ? "Turn off" : "Turn on"}
            </Button>
          ) : null}
        </div>

        {pluginState === "checking" || pluginState === "missing" || pluginState === "unavailable" ? (
          <>
            <p className="m-0 rounded-md border border-dashed border-border/70 px-2.5 py-2 text-[11px] text-muted-foreground">
              {pluginMessage}
            </p>
            {recoveryActions}
          </>
        ) : pluginState === "ready" ? (
          loading && !value ? (
            <p className="m-0 text-[11px] text-muted-foreground">Checking…</p>
          ) : (
            <NowPlayingCard
              value={value}
              controlBusy={busy}
              onControl={(action) => void controlNowPlaying(action)}
              onVolumeChange={(volumePercent) => void changeVolume(volumePercent)}
            />
          )
        ) : (
          <p className="m-0 rounded-md border border-dashed border-border/70 px-2.5 py-2 text-[11px] text-muted-foreground">
            Turn it on to see the song and controls.
          </p>
        )}

        <p className="m-0 border-t border-border/60 pt-2 text-[10px] leading-relaxed text-muted-foreground">
          Only affects this PC.
        </p>
        {error ? <p className="m-0 text-[11px] text-destructive">{error}</p> : null}
      </div>
    </TopbarPanel>
  );
}
