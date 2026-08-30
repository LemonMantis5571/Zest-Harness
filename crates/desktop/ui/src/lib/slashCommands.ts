import type { CommandView } from "./types";

export type SlashMatchParts = {
  prefix: string;
  match: string;
  suffix: string;
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

/** True when the first token is the built-in `/model` command. */
export function isModelSlash(input: string): boolean {
  const name = input.trimStart().match(/^\/([a-z0-9-_]+)/i)?.[1];
  return name?.toLowerCase() === "model";
}

export function isModelCommandName(name: string): boolean {
  return name.toLowerCase() === "model";
}
