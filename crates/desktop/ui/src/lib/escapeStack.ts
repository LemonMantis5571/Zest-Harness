/**
 * What Escape should dismiss, given the stack of open surfaces.
 *
 * The key is not rebindable: it always means "the thing on top". ChatScreen
 * used to skip the Customize / Profile / Usage panel, so Escape closed the
 * panel *and* cancelled a running turn.
 */
export type EscapeAction =
  | "diff"
  | "provider-switch"
  | "model-picker"
  | "settings"
  | "palette"
  | "editing"
  | "shell-panel"
  | "stop-turn";

export function escapeAction(open: {
  diff: boolean;
  providerSwitch: boolean;
  modelPicker: boolean;
  settings: boolean;
  palette: boolean;
  editing: boolean;
  shellPanel: boolean;
  sending: boolean;
}): EscapeAction | null {
  if (open.diff) return "diff";
  if (open.providerSwitch) return "provider-switch";
  if (open.modelPicker) return "model-picker";
  if (open.settings) return "settings";
  if (open.palette) return "palette";
  if (open.editing) return "editing";
  if (open.shellPanel) return "shell-panel";
  if (open.sending) return "stop-turn";
  return null;
}
