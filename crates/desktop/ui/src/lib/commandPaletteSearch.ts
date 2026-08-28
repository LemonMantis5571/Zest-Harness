import type { CommandView, ProjectChats } from "./types";

export type PaletteFilter = "all" | "chats" | "actions" | "settings";

export type PaletteActionInfo = {
  id: string;
  label: string;
  description: string;
  shortcut?: string;
  group?: "action" | "settings";
};

export type ChatHit = {
  id: string;
  title: string;
  projectName: string;
  projectPath: string | null;
  updatedAt: number;
  snippet?: string | null;
};

export type PaletteItem =
  | { kind: "chat"; item: ChatHit }
  | { kind: "action"; item: PaletteActionInfo }
  | { kind: "command"; item: CommandView };

export type PaletteSection = {
  id: string;
  label: string;
  items: PaletteItem[];
};

export const PALETTE_FILTERS: { id: PaletteFilter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "chats", label: "Chats" },
  { id: "actions", label: "Actions" },
  { id: "settings", label: "Settings" },
];

export function matchesQuery(haystack: string, query: string) {
  return !query || haystack.toLowerCase().includes(query);
}

/** First matching window around `query`, collapsed to one line. */
export function matchExcerpt(text: string, query: string, radius = 42): string | null {
  const needle = query.trim().toLowerCase();
  if (!needle) return null;
  const haystack = text.toLowerCase();
  const at = haystack.indexOf(needle);
  if (at < 0) return null;
  const start = Math.max(0, at - radius);
  const end = Math.min(text.length, at + needle.length + radius);
  let snippet = text.slice(start, end).replace(/\s+/g, " ").trim();
  if (!snippet) return null;
  if (start > 0) snippet = `…${snippet}`;
  if (end < text.length) snippet = `${snippet}…`;
  return snippet;
}

export function mergeChatHits(titleHits: ChatHit[], bodyHits: ChatHit[]): ChatHit[] {
  const byId = new Map<string, ChatHit>();
  for (const hit of [...titleHits, ...bodyHits]) {
    const existing = byId.get(hit.id);
    if (!existing) {
      byId.set(hit.id, hit);
      continue;
    }
    if (!existing.snippet && hit.snippet) {
      byId.set(hit.id, { ...existing, snippet: hit.snippet });
    }
  }
  return [...byId.values()].sort((a, b) => b.updatedAt - a.updatedAt);
}

/** Compact relative age like the reference palette (6m, 8h, 2d). */
export function formatPaletteAge(epochSecs: number, nowSecs = Math.floor(Date.now() / 1000)) {
  if (!epochSecs) return "";
  const delta = Math.max(0, nowSecs - epochSecs);
  if (delta < 60) return `${delta}s`;
  if (delta < 3600) return `${Math.floor(delta / 60)}m`;
  if (delta < 86400) return `${Math.floor(delta / 3600)}h`;
  if (delta < 86400 * 14) return `${Math.floor(delta / 86400)}d`;
  try {
    return new Intl.DateTimeFormat(undefined, {
      month: "short",
      day: "numeric",
    }).format(new Date(epochSecs * 1000));
  } catch {
    return "";
  }
}

export function flattenChats(projects: ProjectChats[]): ChatHit[] {
  const hits: ChatHit[] = [];
  for (const project of projects) {
    for (const thread of project.threads) {
      hits.push({
        id: thread.id,
        title: thread.title?.trim() || "Untitled chat",
        projectName: project.path === null ? "No workspace" : project.name,
        projectPath: project.path,
        updatedAt: thread.updatedAt,
      });
    }
  }
  return hits.sort((a, b) => b.updatedAt - a.updatedAt);
}

export function shiftFilter(current: PaletteFilter, delta: number): PaletteFilter {
  const index = PALETTE_FILTERS.findIndex((item) => item.id === current);
  const next = (index + delta + PALETTE_FILTERS.length) % PALETTE_FILTERS.length;
  return PALETTE_FILTERS[next]?.id ?? "all";
}

export function buildPaletteSections(
  filter: PaletteFilter,
  query: string,
  chats: ChatHit[],
  actions: PaletteActionInfo[],
  commands: CommandView[]
): PaletteSection[] {
  const q = query.trim().toLowerCase();
  const searching = q.length > 0;
  const actionItems = actions.filter((item) => (item.group ?? "action") === "action");
  const settingItems = actions.filter((item) => item.group === "settings");

  const chatHits = chats.filter((item) =>
    matchesQuery(`${item.title} ${item.projectName} ${item.snippet ?? ""}`, q)
  );
  const actionHits = actionItems.filter((item) =>
    matchesQuery(`${item.label} ${item.description}`, q)
  );
  const settingHits = settingItems.filter((item) =>
    matchesQuery(`${item.label} ${item.description}`, q)
  );
  const commandHits = commands.filter((item) =>
    matchesQuery(`/${item.name} ${item.description}`, q)
  );

  const chatLimit = filter === "chats" ? 24 : searching ? 12 : 8;
  const sections: PaletteSection[] = [];
  const showChats = filter === "all" || filter === "chats";
  const showActions = filter === "all" || filter === "actions";
  const showSettings = filter === "settings" || (filter === "all" && searching);

  if (showChats && chatHits.length > 0) {
    sections.push({
      id: "chats",
      label: searching ? "Chats" : "Recent chats",
      items: chatHits.slice(0, chatLimit).map((item) => ({ kind: "chat", item })),
    });
  }

  if (showActions && actionHits.length > 0) {
    sections.push({
      id: "actions",
      label: "Actions",
      items: actionHits.map((item) => ({ kind: "action", item })),
    });
  }

  if (showActions && (searching || filter === "actions") && commandHits.length > 0) {
    sections.push({
      id: "commands",
      label: "Commands",
      items: commandHits.slice(0, 16).map((item) => ({ kind: "command", item })),
    });
  }

  if (showSettings && settingHits.length > 0) {
    sections.push({
      id: "settings",
      label: "Settings",
      items: settingHits.map((item) => ({ kind: "action", item })),
    });
  }

  return sections;
}
