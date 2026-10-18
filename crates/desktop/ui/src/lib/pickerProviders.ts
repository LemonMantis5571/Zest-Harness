import { useSyncExternalStore } from "react";

/**
 * Providers the user unchecked in Customize > Models, kept out of the model
 * picker. Stored as the hidden ids rather than the shown ones, so a provider
 * added later appears until the user decides otherwise.
 */
const STORAGE_KEY = "zest.pickerHiddenProviders.v1";
const EMPTY: ReadonlySet<string> = new Set();
const listeners = new Set<() => void>();
let cached: ReadonlySet<string> | undefined;

/** Read a stored value; anything unreadable hides nothing. */
export function parseHiddenPickerProviders(raw: string | null): ReadonlySet<string> {
  if (!raw) return EMPTY;
  try {
    const value: unknown = JSON.parse(raw);
    if (!Array.isArray(value)) return EMPTY;
    const ids = value.filter((id): id is string => typeof id === "string" && id.length > 0);
    return ids.length ? new Set(ids) : EMPTY;
  } catch {
    return EMPTY;
  }
}

function readHiddenPickerProviders(): ReadonlySet<string> {
  if (cached) return cached;
  try {
    cached = parseHiddenPickerProviders(window.localStorage.getItem(STORAGE_KEY));
  } catch {
    cached = EMPTY;
  }
  return cached;
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function setPickerProviderShown(providerId: string, shown: boolean) {
  const next = new Set(readHiddenPickerProviders());
  if (shown) next.delete(providerId);
  else next.add(providerId);
  cached = next.size ? next : EMPTY;
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify([...next].sort()));
  } catch {
    // Keep the choice for this app session when storage is unavailable.
  }
  for (const listener of listeners) listener();
}

/** The provider ids the model picker leaves out. */
export function useHiddenPickerProviders(): ReadonlySet<string> {
  return useSyncExternalStore(subscribe, readHiddenPickerProviders, () => EMPTY);
}
