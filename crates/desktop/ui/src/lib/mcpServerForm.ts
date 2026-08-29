import { isRecord, parseJson } from "./json.ts";

/**
 * Turning the MCP server form into something the backend can store.
 *
 * Separate from the screen because this is the half that can be wrong in ways
 * a user would have to debug: a quoted path silently split in two, or a token
 * pasted where only a variable name belongs.
 */

export type McpServerKind = "stdio" | "http";

export type McpServerDraft = {
  id: string;
  kind: McpServerKind;
  command: string;
  /** One line, shell-style. */
  args: string;
  /** Comma- or space-separated variable names. */
  envVars: string;
  url: string;
  /** Lines of `Header = ENV_VAR`. */
  headers: string;
  /** Full value for the common HTTP Authorization header. */
  authorizationValue: string;
  timeoutSecs: string;
};

export type McpServerSubmission = {
  id: string;
  command: string;
  args: string[];
  url: string;
  headers: Record<string, string>;
  /** Secret values are sent to Rust and stored in the OS credential manager. */
  headerSecrets: Record<string, string>;
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

const HEADER_NAME = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

/**
 * Parse `Header = ENV_VAR` rows. Commas or newlines are fine. The right-hand
 * side is an environment variable name, never the secret itself. The
 * Authorization secret has its own password field below.
 */
export function parseHeaders(
  text: string
): { ok: true; value: Record<string, string> } | { ok: false; error: string } {
  const value: Record<string, string> = {};
  const parts = text
    .split(/[\n,]+/)
    .map((part) => part.trim())
    .filter((part) => part.length > 0);
  for (const part of parts) {
    const eq = part.indexOf("=");
    if (eq <= 0) {
      return { ok: false, error: "Each header is `Name = ENV_VAR`." };
    }
    const name = part.slice(0, eq).trim();
    const envName = part.slice(eq + 1).trim();
    if (!HEADER_NAME.test(name)) {
      return { ok: false, error: `\`${name}\` is not a valid HTTP header name.` };
    }
    if (!ENV_NAME.test(envName)) {
      return {
        ok: false,
        error: "Header values must be environment variable names, not the secret.",
      };
    }
    value[name] = envName;
  }
  return { ok: true, value };
}

export function formatHeaders(headers: Record<string, string | undefined>): string {
  return Object.entries(headers)
    .filter((entry): entry is [string, string] => typeof entry[1] === "string" && entry[1].length > 0)
    .map(([name, envName]) => `${name} = ${envName}`)
    .join("\n");
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

  const timeoutSecs = Number(draft.timeoutSecs);
  if (!Number.isInteger(timeoutSecs) || timeoutSecs < 1 || timeoutSecs > 600) {
    return { ok: false, error: "Timeout must be a whole number of seconds, 1 to 600." };
  }

  if (draft.kind === "http") {
    const url = draft.url.trim();
    if (!url) {
      return { ok: false, error: "Enter the MCP endpoint URL." };
    }
    let parsed: URL;
    try {
      parsed = new URL(url);
    } catch {
      return { ok: false, error: "That URL is not valid." };
    }
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
      return { ok: false, error: "The URL must be http or https." };
    }
    const headers = parseHeaders(draft.headers);
    if (!headers.ok) return headers;
    const authorizationValue = draft.authorizationValue.trim();
    const headerValues = { ...headers.value };
    if (authorizationValue) {
      for (const name of Object.keys(headerValues)) {
        if (name.toLowerCase() === "authorization") delete headerValues[name];
      }
    }
    return {
      ok: true,
      value: {
        id,
        command: "",
        args: [],
        url,
        headers: headerValues,
        headerSecrets: authorizationValue ? { Authorization: authorizationValue } : {},
        envVars: [],
        timeoutSecs,
      },
    };
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

  return {
    ok: true,
    value: {
      id,
      command,
      args: parseArgs(draft.args),
      url: "",
      headers: {},
      headerSecrets: {},
      envVars,
      timeoutSecs,
    },
  };
}

/** Desktop errors arrive as a JSON envelope; show its message, not the JSON. */
export function messageFromError(error: unknown, fallback: string): string {
  const raw = typeof error === "string" ? error : (error as Error)?.message;
  if (!raw) return fallback;
  const parsed = parseJson(raw);
  if (!isRecord(parsed)) return raw;
  return typeof parsed.message === "string" ? parsed.message.trim() || raw : raw;
}
