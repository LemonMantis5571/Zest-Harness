import type { PluginView } from "./types";

export type NowPlayingPluginState =
  | "checking"
  | "missing"
  | "unavailable"
  | "disabled"
  | "ready";

export function nowPlayingPluginState(
  checked: boolean,
  plugin: PluginView | null
): NowPlayingPluginState {
  if (!checked) return "checking";
  if (!plugin) return "missing";
  if (!plugin.available) return "unavailable";
  if (!plugin.enabled) return "disabled";
  return "ready";
}

/**
 * Whether the topbar should carry the music control at all.
 *
 * The header is for playback, not for discovering extras. Until Now Playing is
 * installed, available, and turned on, there is no track to show, so the
 * control stays out of the topbar. Checking, a missing folder, a broken
 * install, and an add-on that is still off all hide the same way; Customize >
 * Extras is where those states are explained and turned on.
 */
export function nowPlayingButtonVisible(state: NowPlayingPluginState): boolean {
  return state === "ready";
}
