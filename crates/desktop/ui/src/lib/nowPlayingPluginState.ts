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
 * A machine with no add-on installed reports `missing`, and there is nothing
 * for the button to control, so it stays out of the topbar entirely. `checking`
 * is hidden for the same reason: a fresh install would otherwise flash a button
 * that immediately disappears. An add-on that is installed but broken keeps its
 * button, because the panel behind it is what explains the problem.
 */
export function nowPlayingButtonVisible(state: NowPlayingPluginState): boolean {
  return state !== "checking" && state !== "missing";
}
