export const WALLPAPER_CHANGED_EVENT = "zest:wallpaper-changed";

export function notifyWallpaperChanged() {
  window.dispatchEvent(new Event(WALLPAPER_CHANGED_EVENT));
}
