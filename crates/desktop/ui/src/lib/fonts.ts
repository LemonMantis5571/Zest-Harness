export type FontCategory = "sans" | "serif" | "mono" | "variable";

export type AppFont = {
  id: string;
  name: string;
  category: FontCategory;
  fontFamily: string;
  description: string;
  previewText?: string;
};

export const AVAILABLE_FONTS: AppFont[] = [
  {
    id: "geist",
    name: "Geist",
    category: "sans",
    fontFamily: '"Geist Variable", "Inter", -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif',
    description: "Zest default font for dense interfaces.",
  },
  {
    id: "abc-arizona",
    name: "ABC Arizona",
    category: "variable",
    fontFamily: '"ABC Arizona", "ABC Arizona Sans", "Arizona Sans", "Plus Jakarta Sans", -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif',
    description: "Variable typeface with humanist curves.",
  },
  {
    id: "inter",
    name: "Inter",
    category: "sans",
    fontFamily: '"Inter Variable", "Inter", -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif',
    description: "Neutral sans-serif for interface text.",
  },
  {
    id: "plus-jakarta",
    name: "Plus Jakarta Sans",
    category: "sans",
    fontFamily: '"Plus Jakarta Sans Variable", "Plus Jakarta Sans", -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif',
    description: "Geometric sans-serif for interface text.",
  },
  {
    id: "jetbrains-mono",
    name: "JetBrains Mono",
    category: "mono",
    fontFamily: '"JetBrains Mono Variable", "JetBrains Mono", "SF Mono", ui-monospace, Consolas, monospace',
    description: "Monospace font with distinct symbols.",
  },
  {
    id: "fira-code",
    name: "Fira Code",
    category: "mono",
    fontFamily: '"Fira Code Variable", "Fira Code", "SF Mono", ui-monospace, Consolas, monospace',
    description: "Monospace font for code and technical text.",
  },
  {
    id: "system",
    name: "System UI",
    category: "sans",
    fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif',
    description: "Native operating system typography (SF Pro on macOS, Segoe UI on Windows).",
  },
];

export const DEFAULT_FONT_ID = "geist";
export const FONT_STORAGE_KEY = "zest.selected_font";

type FontLoader = () => Promise<unknown>;

// Keep the default shell fonts in the initial stylesheet. Optional families are
// split into CSS chunks so opening Settings does not fetch every preview font.
const OPTIONAL_FONT_LOADERS: Partial<Record<string, FontLoader>> = {
  "abc-arizona": () => import("../fonts/arizona.css"),
  inter: () => import("@fontsource-variable/inter"),
  "plus-jakarta": () => import("@fontsource-variable/plus-jakarta-sans"),
  "jetbrains-mono": () => import("@fontsource-variable/jetbrains-mono"),
  "fira-code": () => import("@fontsource-variable/fira-code"),
};

const loadedFonts = new Set(["geist", "system"]);
const loadingFonts = new Map<string, Promise<boolean>>();

export function getFontById(id: string): AppFont {
  return AVAILABLE_FONTS.find((font) => font.id === id) ?? AVAILABLE_FONTS[0];
}

/** Load an optional font's CSS once, returning false when the bundle fails. */
export function ensureFontLoaded(fontId: string): Promise<boolean> {
  const font = getFontById(fontId);
  if (loadedFonts.has(font.id)) {
    return Promise.resolve(true);
  }

  const existingLoad = loadingFonts.get(font.id);
  if (existingLoad) {
    return existingLoad;
  }

  const loader = OPTIONAL_FONT_LOADERS[font.id];
  if (!loader) {
    loadedFonts.add(font.id);
    return Promise.resolve(true);
  }

  const load = loader()
    .then(() => {
      loadedFonts.add(font.id);
      loadingFonts.delete(font.id);
      return true;
    })
    .catch(() => {
      // Do not cache failures: a transient WebView/asset error can be retried.
      loadingFonts.delete(font.id);
      return false;
    });
  loadingFonts.set(font.id, load);
  return load;
}

export function getSavedFontId(): string {
  try {
    if (typeof localStorage !== "undefined") {
      const saved = localStorage.getItem(FONT_STORAGE_KEY);
      if (saved && AVAILABLE_FONTS.some((font) => font.id === saved)) {
        return saved;
      }
    }
  } catch {
    // Ignore storage errors in restricted contexts
  }
  return DEFAULT_FONT_ID;
}

export function applyFont(fontId: string): AppFont {
  const font = getFontById(fontId);
  try {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(FONT_STORAGE_KEY, font.id);
    }
  } catch {
    // Ignore storage errors
  }

  if (typeof document !== "undefined") {
    document.documentElement.style.setProperty("--app-font-family", font.fontFamily);
    document.documentElement.setAttribute("data-font", font.id);

    void ensureFontLoaded(font.id).then((loaded) => {
      if (!loaded && document.documentElement.getAttribute("data-font") === font.id) {
        const fallback = getFontById(DEFAULT_FONT_ID);
        document.documentElement.style.setProperty("--app-font-family", fallback.fontFamily);
        document.documentElement.setAttribute("data-font", fallback.id);
      }
    });
  } else {
    void ensureFontLoaded(font.id);
  }

  return font;
}
