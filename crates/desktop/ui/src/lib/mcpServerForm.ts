/**
 * Turning the MCP server form into something the backend can store.
 *
 * Separate from the screen because this is the half that can be wrong in ways
 * a user would have to debug: a quoted path silently split in two, or a token
 * pasted where only a variable name belongs.
 */

export type McpServerDraft = {
  id: string;
  command: string;
  /** One line, shell-style. */
  args: string;
  /** Comma- or space-separated variable names. */
  envVars: string;
  timeoutSecs: string;
};

export type McpServerSubmission = {
  id: string;
  command: string;
  args: string[];
  envVars: string[];
  timeoutSecs: number;
};

/**
 * Split an argument line the way a shell would for the simple cases.
 *
 * Quoted runs stay together so a path containing a space survives. Nothing
 * else is interpreted — the command is spawned directly, so there is no shell
 * whose behaviour would be worth imitating further.
 */
export function parseArgs(line: string): string[] {
  const args: string[] = [];
  let current = "";
  let quote: '"' | "'" | null = null;
  let quoted = false;
  for (const char of line) {
    if (quote) {
      if (char === quote) quote = null;
      else current += char;
      continue;
    }
    if (char === '"' || char === "'") {
      quote = char;
      quoted = true;
      continue;
    }
    if (/\s/.test(char)) {
      // `quoted` keeps a deliberately empty argument (`""`), which some
      // servers use for a positional placeholder.
      if (current || quoted) args.push(current);
      current = "";
      quoted = false;
      continue;
    }
    current += char;
  }
  if (current || quoted) args.push(current);
  return args;
}

export function parseEnvVars(line: string): string[] {
  return line
    .split(/[,\s]+/)
    .map((name) => name.trim())
    .filter((name) => name.length > 0);
}

/**
 * Check a draft, or say what is wrong in the words the form will show.
 *
 * The backend validates the same things — it has to, since `zest.toml` can be
 * edited by hand — but a message from here arrives before the write and can
 * point at the field.
 */
export function validateMcpServerDraft(
  draft: McpServerDraft
): { ok: true; value: McpServerSubmission } | { ok: false; error: string } {
  const id = draft.id.trim();
  if (!id) return { ok: false, error: "Give the server a name." };
  if (!/^[A-Za-z0-9_-]+$/.test(id)) {
    return { ok: false, error: "Use only letters, numbers, - and _ in the name." };
  }

  const command = draft.command.trim();
  if (!command) {
    return { ok: false, error: "Enter the command that starts the server." };
  }

  const envVars = parseEnvVars(draft.envVars);
  const withValue = envVars.find((name) => name.includes("="));
  if (withValue) {
    return {
      ok: false,
      error: "List variable names only — the value stays in your environment.",
    };
  }

  const timeoutSecs = Number(draft.timeoutSecs);
  if (!Number.isInteger(timeoutSecs) || timeoutSecs < 1 || timeoutSecs > 600) {
    return { ok: false, error: "Timeout must be a whole number of seconds, 1 to 600." };
  }

  return {
    ok: true,
    value: { id, command, args: parseArgs(draft.args), envVars, timeoutSecs },
  };
}

/** Desktop errors arrive as a JSON envelope; show its message, not the JSON. */
export function messageFromError(error: unknown, fallback: string): string {
  const raw = typeof error === "string" ? error : (error as Error)?.message;
  if (!raw) return fallback;
  try {
    const parsed = JSON.parse(raw) as { message?: string };
    return parsed.message?.trim() || raw;
  } catch {
    return raw;
  }
}
