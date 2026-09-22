import { useSyncExternalStore } from "react";

const STORAGE_KEY = "zest.responseBlockStreaming.v1";
const listeners = new Set<() => void>();
let cachedValue: boolean | undefined;

function readResponseBlockStreaming(): boolean {
  if (cachedValue !== undefined) return cachedValue;

  try {
    const saved = window.localStorage.getItem(STORAGE_KEY);
    cachedValue = saved == null ? true : saved === "true";
  } catch {
    cachedValue = true;
  }

  return cachedValue;
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function setResponseBlockStreaming(enabled: boolean) {
  cachedValue = enabled;
  try {
    window.localStorage.setItem(STORAGE_KEY, String(enabled));
  } catch {
    // Keep the preference for this app session when storage is unavailable.
  }
  for (const listener of listeners) listener();
}

/** Defaults to block streaming until the user turns it off in Settings. */
export function useResponseBlockStreaming(): boolean {
  return useSyncExternalStore(subscribe, readResponseBlockStreaming, () => true);
}
