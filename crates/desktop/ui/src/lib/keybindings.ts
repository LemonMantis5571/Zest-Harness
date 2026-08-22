/**
 * Keyboard commands, their bindings, and the storage behind both.
 *
 * One registry rather than a chord test per component. The hardcoded blocks
 * this replaces could not answer "what is bound to Ctrl+B" without reading
 * every file, which is exactly the question a rebinding UI has to answer, and
 * the question that decides whether a new shortcut collides with an old one.
 *
 * `Mod` is Ctrl everywhere and Cmd on macOS. Zest's existing shortcuts already
 * treated the two as interchangeable, so binding them separately would create a
 * distinction users never asked for.
 */

export type CommandId =
  | "chat.new"
  | "chat.stop"
  | "focus.composer"
  | "view.sidebar"
  | "view.profile"
  | "view.usage"
  | "view.customize"
  | "view.settings"
  | "view.shortcuts"
  | "view.provider"
  | "view.palette";

/** Groups in the shortcuts editor, in display order. */
export const SECTIONS = ["Navigation", "Chat", "Focus"] as const;
export type Section = (typeof SECTIONS)[number];

export type CommandDef = {
  id: CommandId;
  label: string;
  section: Section;
  /** Empty string means "no default"; the user may still bind one. */
  defaultChord: string;
  /** Shown under the label in the editor. */
  hint?: string;
};

export const COMMANDS: CommandDef[] = [
  {
    id: "view.sidebar",
    label: "Toggle chat history",
    section: "Navigation",
    defaultChord: "Mod+B",
  },
  { id: "view.profile", label: "Open profile", section: "Navigation", defaultChord: "Mod+Shift+P" },
  {
    id: "view.usage",
    label: "Open usage",
    section: "Navigation",
    defaultChord: "Mod+Shift+U",
    hint: "Tokens and estimated cost over time",
  },
  {
    id: "view.customize",
    label: "Open Customize",
    section: "Navigation",
    // Deliberately paired with Settings' `Mod+,` rather than the obvious
    // `Mod+Shift+C`, which Chromium claims for the element inspector.
    defaultChord: "Mod+Shift+,",
    hint: "MCP servers, skills, extras, rules, and shortcuts",
  },
  { id: "view.settings", label: "Open settings", section: "Navigation", defaultChord: "Mod+," },
  {
    id: "view.palette",
    label: "Command palette",
    section: "Navigation",
    defaultChord: "Mod+K",
    hint: "Search actions and backend commands",
  },
  {
    id: "view.shortcuts",
    label: "Keyboard shortcuts",
    section: "Navigation",
    defaultChord: "Mod+Shift+K",
    hint: "Opens Customize at this section",
  },
  {
    id: "view.provider",
    label: "Switch provider",
    section: "Navigation",
    defaultChord: "Mod+Shift+M",
  },
  { id: "chat.new", label: "New chat", section: "Chat", defaultChord: "Mod+N" },
  {
    id: "chat.stop",
    label: "Stop the current turn",
    section: "Chat",
    defaultChord: "Mod+.",
    hint: "Escape also stops a running turn",
  },
  {
    id: "focus.composer",
    label: "Focus the message box",
    section: "Focus",
    defaultChord: "/",
    hint: "Only when not already typing",
  },
];

const BY_ID = new Map(COMMANDS.map((c) => [c.id, c]));

export function commandById(id: CommandId): CommandDef | undefined {
  return BY_ID.get(id);
}

/* -------------------------------------------------------------------------- */
/* Chords                                                                      */
/* -------------------------------------------------------------------------- */

/** Modifier order is fixed so two spellings of one chord compare equal. */
const MOD_ORDER = ["Mod", "Alt", "Shift"] as const;

/** Keys that are only ever modifiers — never a chord on their own. */
const MODIFIER_KEYS = new Set(["Control", "Meta", "Alt", "Shift", "AltGraph"]);

/**
 * A keydown as a canonical chord, or `null` when the event is only a modifier
 * being held down.
 */
export function chordFromEvent(event: KeyboardEvent): string | null {
  if (MODIFIER_KEYS.has(event.key)) return null;

  const parts: string[] = [];
  if (event.ctrlKey || event.metaKey) parts.push("Mod");
  if (event.altKey) parts.push("Alt");
  if (event.shiftKey) parts.push("Shift");
  parts.push(normalizeKey(event.key));
  return parts.join("+");
}

/**
 * One spelling per physical key.
 *
 * Letters upper-case so Shift does not produce a second, unequal chord, and the
 * space bar spelled rather than left as a literal space that a chord string
 * could not round-trip.
 */
function normalizeKey(key: string): string {
  if (key === " ") return "Space";
  if (key.length === 1) return key.toUpperCase();
  return key;
}

/** Parse a stored chord, tolerating case and modifier order. */
export function parseChord(chord: string): { mods: string[]; key: string } | null {
  const raw = chord
    .split("+")
    .map((p) => p.trim())
    .filter(Boolean);
  if (raw.length === 0) return null;

  const mods: string[] = [];
  let key = "";
  for (const part of raw) {
    const lower = part.toLowerCase();
    if (lower === "mod" || lower === "ctrl" || lower === "control" || lower === "cmd") {
      if (!mods.includes("Mod")) mods.push("Mod");
    } else if (lower === "alt" || lower === "option") {
      if (!mods.includes("Alt")) mods.push("Alt");
    } else if (lower === "shift") {
      if (!mods.includes("Shift")) mods.push("Shift");
    } else {
      key = normalizeKey(part);
    }
  }
  if (!key) return null;
  return { mods: MOD_ORDER.filter((m) => mods.includes(m)), key };
}

/** Canonical spelling, so equality is string equality. */
export function normalizeChord(chord: string): string | null {
  const parsed = parseChord(chord);
  if (!parsed) return null;
  return [...parsed.mods, parsed.key].join("+");
}

/** How the chord is written for a human, given the platform. */
export function formatChord(chord: string, mac = isMac()): string {
  const parsed = parseChord(chord);
  if (!parsed) return "";
  const mods = parsed.mods.map((m) => {
    if (m === "Mod") return mac ? "⌘" : "Ctrl";
    if (m === "Alt") return mac ? "⌥" : "Alt";
    return mac ? "⇧" : "Shift";
  });
  return [...mods, displayKey(parsed.key)].join(mac ? "" : "+");
}

function displayKey(key: string): string {
  if (key === "ArrowUp") return "↑";
  if (key === "ArrowDown") return "↓";
  if (key === "ArrowLeft") return "←";
  if (key === "ArrowRight") return "→";
  return key;
}

export function isMac(): boolean {
  if (typeof navigator === "undefined") return false;
  return /mac|iphone|ipad/i.test(navigator.platform || navigator.userAgent);
}

/**
 * Whether a chord is safe to bind.
 *
 * A bare printable key is allowed — `/` already focuses the composer — but only
 * because dispatch ignores unmodified keys while a field has focus. Enter and
 * Tab are refused outright: rebinding them breaks sending a message and moving
 * focus, which would make the editor itself hard to escape.
 */
export function chordIsBindable(chord: string): { ok: true } | { ok: false; reason: string } {
  const parsed = parseChord(chord);
  if (!parsed) return { ok: false, reason: "Not a usable key combination" };
  if (parsed.mods.length === 0) {
    if (["Enter", "Tab", "Escape", "Space", "Backspace"].includes(parsed.key)) {
      return { ok: false, reason: `${parsed.key} cannot be rebound` };
    }
  }
  if (parsed.key === "Escape") {
    return { ok: false, reason: "Escape always closes the current surface" };
  }
  return { ok: true };
}

/* -------------------------------------------------------------------------- */
/* Storage                                                                     */
/* -------------------------------------------------------------------------- */

const STORAGE_KEY = "zest.keybindings";

/** Command id to chord. An explicit empty string means deliberately unbound. */
export type Bindings = Partial<Record<CommandId, string>>;

export function defaultBindings(): Bindings {
  const out: Bindings = {};
  for (const command of COMMANDS) out[command.id] = command.defaultChord;
  return out;
}

/**
 * Stored overrides merged onto the defaults.
 *
 * Defaults are re-applied on every load rather than baked into storage, so a
 * later release can change a default and have it reach anyone who never touched
 * that particular binding.
 */
export function loadBindings(): Bindings {
  const bindings = defaultBindings();
  if (typeof localStorage === "undefined") return bindings;
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return bindings;
    const saved = JSON.parse(raw) as Record<string, unknown>;
    for (const command of COMMANDS) {
      const value = saved[command.id];
      if (typeof value !== "string") continue;
      // "" is a real choice: the user cleared this binding.
      bindings[command.id] = value === "" ? "" : (normalizeChord(value) ?? command.defaultChord);
    }
  } catch {
    /* Unreadable storage falls back to defaults rather than blocking startup. */
  }
  return bindings;
}

/** Persist only what differs from the defaults. */
export function saveBindings(bindings: Bindings): void {
  if (typeof localStorage === "undefined") return;
  const overrides: Record<string, string> = {};
  for (const command of COMMANDS) {
    const chord = bindings[command.id] ?? command.defaultChord;
    if (chord !== command.defaultChord) overrides[command.id] = chord;
  }
  try {
    if (Object.keys(overrides).length === 0) localStorage.removeItem(STORAGE_KEY);
    else localStorage.setItem(STORAGE_KEY, JSON.stringify(overrides));
  } catch {
    /* A full or blocked store must not break the settings panel. */
  }
}

/**
 * Commands sharing a chord, keyed by chord.
 *
 * Reported rather than prevented: the editor shows the clash and lets the user
 * resolve it, which is friendlier than silently refusing the key they pressed.
 */
export function conflicts(bindings: Bindings): Map<string, CommandId[]> {
  const byChord = new Map<string, CommandId[]>();
  for (const command of COMMANDS) {
    const chord = bindings[command.id];
    if (!chord) continue;
    const list = byChord.get(chord) ?? [];
    list.push(command.id);
    byChord.set(chord, list);
  }
  for (const [chord, ids] of byChord) {
    if (ids.length < 2) byChord.delete(chord);
  }
  return byChord;
}

/** The command a chord should run, or `null`. First match in registry order. */
export function commandFor(bindings: Bindings, chord: string): CommandId | null {
  // Guard the empty chord explicitly: `"" === ""` would otherwise fire whichever
  // command the user had deliberately unbound.
  if (!chord) return null;
  for (const command of COMMANDS) {
    if (bindings[command.id] === chord) return command.id;
  }
  return null;
}
