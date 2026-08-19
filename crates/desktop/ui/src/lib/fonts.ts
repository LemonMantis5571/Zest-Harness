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
    description: "Zest default. Engineered for high-density developer interfaces and crisp legibility.",
  },
  {
    id: "abc-arizona",
    name: "ABC Arizona",
    category: "variable",
    fontFamily: '"ABC Arizona", "ABC Arizona Sans", "Arizona Sans", "Plus Jakarta Sans", -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif',
    description: "Distinctive variable typeface with warm humanist curves and contemporary flair.",
  },
  {
    id: "inter",
    name: "Inter",
    category: "sans",
    fontFamily: '"Inter Variable", "Inter", -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif',
    description: "Ubiquitous, highly neutral UI standard optimized for readability on computer screens.",
  },
  {
    id: "plus-jakarta",
    name: "Plus Jakarta Sans",
    category: "sans",
    fontFamily: '"Plus Jakarta Sans Variable", "Plus Jakarta Sans", -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif',
    description: "Modern geometric sans-serif with refined curves and balanced proportions.",
  },
  {
    id: "jetbrains-mono",
    name: "JetBrains Mono",
    category: "mono",
    fontFamily: '"JetBrains Mono Variable", "JetBrains Mono", "SF Mono", ui-monospace, Consolas, monospace',
    description: "Developer-favorite monospace typeface with clear symbol distinction.",
  },
  {
    id: "fira-code",
    name: "Fira Code",
    category: "mono",
    fontFamily: '"Fira Code Variable", "Fira Code", "SF Mono", ui-monospace, Consolas, monospace',
    description: "Clean monospaced font designed for programming workflows and technical text.",
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

export function getFontById(id: string): AppFont {
  return AVAILABLE_FONTS.find((font) => font.id === id) ?? AVAILABLE_FONTS[0];
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
  }

  return font;
}
