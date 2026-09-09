import type { CommandView } from "./types";

export type SlashMatchParts = {
  prefix: string;
  match: string;
  suffix: string;
};

export type SlashToken = {
  query: string;
  start: number;
  end: number;
};

/** Split `name` so the typed `/` query can be coloured like a match. */
export function splitSlashMatch(name: string, query: string): SlashMatchParts {
  const needle = query.trim();
  if (!needle) return { prefix: "", match: "", suffix: name };
  const at = name.toLowerCase().indexOf(needle.toLowerCase());
  if (at < 0) return { prefix: "", match: "", suffix: name };
  return {
    prefix: name.slice(0, at),
    match: name.slice(at, at + needle.length),
    suffix: name.slice(at + needle.length),
  };
}

export function filterSlashCommands(
  commands: CommandView[],
  typed: string
): CommandView[] {
  const q = typed.toLowerCase();
  return commands.filter((command) => command.name.toLowerCase().startsWith(q));
}

/**
 * Find the slash token immediately before the caret.
 *
 * Commands are separate, whitespace-delimited tokens. Looking only at the
 * text before the caret lets a draft contain more than one command and keeps
 * paths such as `/etc/hosts` from opening the palette while they are typed.
 */
export function slashTokenAt(input: string, caret: number): SlashToken | null {
  const position = Math.max(0, Math.min(caret, input.length));
  let start = position;

  while (start > 0 && !/\s/u.test(input[start - 1] ?? "")) {
    start -= 1;
  }

  if (input[start] !== "/") return null;
  if (start > 0 && !/\s/u.test(input[start - 1] ?? "")) return null;

  const token = input.slice(start, position);
  const match = /^\/([a-z0-9_-]*)$/i.exec(token);
  if (!match) return null;

  return {
    query: match[1] ?? "",
    start,
    end: position,
  };
}

/** True when the first token is the built-in `/model` command. */
export function isModelSlash(input: string): boolean {
  const name = input.trimStart().match(/^\/([a-z0-9-_]+)/i)?.[1];
  return name?.toLowerCase() === "model";
}

export function isModelCommandName(name: string): boolean {
  return name.toLowerCase() === "model";
}
