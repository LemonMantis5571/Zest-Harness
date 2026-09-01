import { invoke } from "@tauri-apps/api/core";

/** Push the selected palette onto native window chrome. No-ops in the browser. */
export function setWindowChrome(background: string, appearance: string) {
  return invoke<void>("set_window_chrome", { background, appearance });
}

export function openExternalUrl(url: string) {
  return invoke<void>("open_external_url", { url });
}
