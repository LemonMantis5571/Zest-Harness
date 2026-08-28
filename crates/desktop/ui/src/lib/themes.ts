export type ThemeAppearance = "dark" | "light";

export type ThemeSwatches = {
  background: string;
  sidebar: string;
  card: string;
  primary: string;
};

export type AppTheme = {
  id: string;
  name: string;
  description: string;
  appearance: ThemeAppearance;
  swatches: ThemeSwatches;
};

/**
 * Built-in palettes. Token values live in `index.css` under `[data-theme]`.
 * This registry is the picker, storage, and appearance switch.
 *
 * Oceanic maps oceanic-ui's Ocean/Metal grammar. Nights maps a night-sky
 * canvas: indigo-purple chrome, magenta actions, cyan focus.
 */
export const AVAILABLE_THEMES: AppTheme[] = [
  {
    id: "zest",
    name: "Zest",
    description: "The default dark canvas.",
    appearance: "dark",
    swatches: {
      background: "#0c0c0e",
      sidebar: "#121314",
      card: "#141516",
      primary: "#5e6ad2",
    },
  },
  {
    id: "nights",
    name: "Nights",
    description: "Midnight purple, magenta, and gold.",
    appearance: "dark",
    swatches: {
      background: "#12081c",
      sidebar: "#160c28",
      card: "#1b102c",
      primary: "#e4458c",
    },
  },
  {
    id: "oceanic",
    name: "Oceanic",
    description: "Clear-sky blues inspired by Ocean, Java's Metal look.",
    appearance: "light",
    swatches: {
      background: "#eaf6ff",
      sidebar: "#eef8ff",
      card: "#f7fcff",
      primary: "#1367c7",
    },
  },
];

export const DEFAULT_THEME_ID = "zest";
export const THEME_STORAGE_KEY = "zest.selected_theme";
export const THEME_CHANGED_EVENT = "zest:theme-changed";

export function getThemeById(id: string): AppTheme {
  return AVAILABLE_THEMES.find((theme) => theme.id === id) ?? AVAILABLE_THEMES[0];
}

export function getSavedThemeId(): string {
  try {
    if (typeof localStorage !== "undefined") {
      const saved = localStorage.getItem(THEME_STORAGE_KEY);
      if (saved && AVAILABLE_THEMES.some((theme) => theme.id === saved)) {
        return saved;
      }
    }
  } catch {
    // Ignore storage errors in restricted contexts
  }
  return DEFAULT_THEME_ID;
}

export function getSavedTheme(): AppTheme {
  return getThemeById(getSavedThemeId());
}

/** Light heads on a dark card; dark heads on a light card. */
export function blobatarToneFor(appearance: ThemeAppearance): number {
  return appearance === "dark" ? 0.2 : 0.82;
}

export function subscribeThemeChange(listener: () => void): () => void {
  if (typeof window === "undefined") return () => {};
  window.addEventListener(THEME_CHANGED_EVENT, listener);
  return () => window.removeEventListener(THEME_CHANGED_EVENT, listener);
}

function syncColorSchemeMeta(appearance: ThemeAppearance) {
  const meta = document.querySelector('meta[name="color-scheme"]');
  if (meta) meta.setAttribute("content", appearance);
}

/**
 * Paint a theme onto the document. The boot script in `index.html` applies the
 * same `data-theme` / `.dark` switch before CSS so the first frame matches.
 */
export function applyTheme(themeId: string): AppTheme {
  const theme = getThemeById(themeId);
  try {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(THEME_STORAGE_KEY, theme.id);
    }
  } catch {
    // Ignore storage errors
  }

  if (typeof document !== "undefined") {
    const root = document.documentElement;
    root.dataset.theme = theme.id;
    root.classList.toggle("dark", theme.appearance === "dark");
    root.style.colorScheme = theme.appearance;
    // The boot script may have pinned a paint colour; CSS owns it after this.
    root.style.removeProperty("background");
    document.body?.style.removeProperty("background");
    syncColorSchemeMeta(theme.appearance);
  }

  if (typeof window !== "undefined") {
    window.dispatchEvent(new Event(THEME_CHANGED_EVENT));
  }

  return theme;
}
