import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";

import { fallbackOnFailure } from "@/lib/backgroundFailure";

let permissionPromise: Promise<boolean> | null = null;

export function isWindowActive() {
  return (
    typeof document !== "undefined" &&
    document.visibilityState === "visible" &&
    document.hasFocus()
  );
}

/** Native focus state is authoritative when the desktop window is minimized. */
export async function isWindowActuallyActive() {
  if (isTauri()) {
    try {
      const window = getCurrentWindow();
      return (await window.isFocused()) && !(await window.isMinimized());
    } catch {
      return isWindowActive();
    }
  }
  return isWindowActive();
}

async function notificationPermission() {
  if (permissionPromise) return permissionPromise;
  permissionPromise = (async () => {
    if (isTauri()) {
      let granted = await isPermissionGranted();
      if (!granted) granted = (await requestPermission()) === "granted";
      return granted;
    }

    if (!("Notification" in window)) return false;
    if (Notification.permission === "default") {
      return (await Notification.requestPermission()) === "granted";
    }
    return Notification.permission === "granted";
  })().catch((error) =>
    fallbackOnFailure(error, false, "request notification permission")
  );
  return permissionPromise;
}

/** Send an OS notification when Zest is not the active window. */
export async function notifyWhenAway(title: string, body: string) {
  if (await isWindowActuallyActive()) return false;
  if (!(await notificationPermission())) return false;

  try {
    if (isTauri()) {
      await sendNotification({ title, body });
    } else {
      new Notification(title, { body });
    }
    return true;
  } catch {
    return false;
  }
}
