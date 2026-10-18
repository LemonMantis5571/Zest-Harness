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

/**
 * Send an OS notification when Zest is not the active window.
 *
 * `onClick` runs when the notification is clicked, where the platform reports
 * that. The Web Notification API does. The Tauri notification plugin does not
 * on desktop: it shows only title, body, icon and sound, and its `onAction`
 * listener is mobile-only, so in the desktop app a click just raises the
 * window (where the OS does so) without opening the chat.
 */
export async function notifyWhenAway(
  title: string,
  body: string,
  { onClick, isCurrent = () => true }: {
    onClick?: () => void;
    isCurrent?: () => boolean;
  } = {}
) {
  if ((await isWindowActuallyActive()) || !isCurrent()) return false;
  if (!(await notificationPermission())) return false;
  if ((await isWindowActuallyActive()) || !isCurrent()) return false;

  try {
    if (isTauri()) {
      await sendNotification({ title, body });
    } else {
      const notification = new Notification(title, { body });
      if (onClick) {
        notification.onclick = () => {
          window.focus();
          notification.close();
          onClick();
        };
      }
    }
    return true;
  } catch {
    return false;
  }
}
