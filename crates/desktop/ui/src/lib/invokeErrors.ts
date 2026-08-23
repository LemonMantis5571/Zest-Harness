type DesktopErrorPayload = {
  code?: unknown;
  message?: unknown;
  details?: unknown;
};

export type ConversationProviderChoice = {
  id: string;
  label: string;
  model: string;
};

export type ConversationRecovery =
  | {
      kind: "unknown_owner";
      threadId: string;
      providers: ConversationProviderChoice[];
    }
  | {
      kind: "owner_unavailable";
      threadId: string;
      providerId: string;
      providerLabel: string;
      configured: boolean;
      providers: ConversationProviderChoice[];
    }
  | {
      kind: "new_chat_unavailable";
      threadId: null;
      providerId: string;
      providerLabel: string;
      configured: boolean;
      providers: ConversationProviderChoice[];
    };

function parseDesktopError(error: unknown): DesktopErrorPayload | null {
  const raw = String(error);
  try {
    const start = raw.indexOf("{");
    const end = raw.lastIndexOf("}");
    if (start >= 0 && end > start) {
      const parsed = JSON.parse(raw.slice(start, end + 1)) as DesktopErrorPayload;
      if (parsed && typeof parsed === "object") return parsed;
    }
  } catch {
    // Keep the raw value available for internal classification only.
  }
  return null;
}

export function busyTurnMessage(error: unknown): string {
  const text = rawInvokeError(error).replace(/^busy:\s*/i, "").trim();
  if (text && !/^busy\b/i.test(text) && !text.toLowerCase().includes("already in progress")) {
    return text.charAt(0).toUpperCase() + text.slice(1);
  }
  return "This chat is still working. Switch chats or wait for it to finish.";
}

export function rawInvokeError(error: unknown): string {
  const raw = String(error);
  const parsed = parseDesktopError(error);
  if (typeof parsed?.message === "string" && parsed.message) {
    return typeof parsed.code === "string" && parsed.code
      ? parsed.code + ": " + parsed.message
      : parsed.message;
  }
  return raw;
}

function providerChoices(value: unknown): ConversationProviderChoice[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((item) => {
    if (!item || typeof item !== "object") return [];
    const record = item as Record<string, unknown>;
    if (typeof record.id !== "string" || typeof record.label !== "string") return [];
    return [
      {
        id: record.id,
        label: record.label,
        model: typeof record.model === "string" ? record.model : "",
      },
    ];
  });
}

/** Read the actionable provider ownership state attached by the desktop backend. */
export function conversationRecovery(error: unknown): ConversationRecovery | null {
  const parsed = parseDesktopError(error);
  const details =
    parsed?.details && typeof parsed.details === "object"
      ? (parsed.details as Record<string, unknown>)
      : null;
  if (!details || typeof parsed?.code !== "string") return null;

  const threadId = typeof details.threadId === "string" ? details.threadId : "";
  const providers = providerChoices(details.availableProviders);
  if (parsed.code === "thread_provider_unknown" && threadId) {
    return { kind: "unknown_owner", threadId, providers };
  }

  const providerId = typeof details.providerId === "string" ? details.providerId : "";
  const providerLabel =
    typeof details.providerLabel === "string" ? details.providerLabel : providerId;
  if (parsed.code === "provider_unavailable" && providerId) {
    const recovery = {
      providerId,
      providerLabel,
      configured: details.configured === true,
      providers,
    };
    return threadId
      ? { kind: "owner_unavailable", threadId, ...recovery }
      : { kind: "new_chat_unavailable", threadId: null, ...recovery };
  }
  return null;
}

/** Marker the desktop backend stamps on every "this folder will not work" failure. */
const WORKSPACE_NOT_WRITABLE = "workspace_not_writable";

/**
 * Phrases that mean the filesystem said no, across the platforms Zest ships to.
 *
 * `"access denied"` on its own used to be the entire test — and Windows says
 * "Access **is** denied.", which contains no such substring. That one gap is why
 * a first run that defaulted to the read-only install directory reached the
 * picker as an unattributed "Something went wrong. Try again."
 */
const PERMISSION_PHRASES = [
  "access is denied",
  "access denied",
  "permission denied",
  "read-only file system",
  "os error 5",
  "os error 13",
];

/** Keeps an unrelated permission failure from being read as a folder problem. */
const STORAGE_HINTS = [".zest", "dir", "directory", "file", "path", "save", "write"];

/**
 * Is this a project-folder problem rather than a provider problem?
 *
 * Worth separating because the remedy has nothing to do with the account: no
 * amount of reconnecting fixes a folder Zest is not allowed to write in, and
 * showing the failure under the provider row makes a working sign-in look
 * broken.
 */
export function isWorkspaceProblem(error: unknown): boolean {
  const message = rawInvokeError(error).toLowerCase();
  if (message.includes(WORKSPACE_NOT_WRITABLE)) return true;
  return (
    PERMISSION_PHRASES.some((phrase) => message.includes(phrase)) &&
    STORAGE_HINTS.some((hint) => message.includes(hint))
  );
}

/**
 * What to tell someone who has no idea what a working directory is.
 *
 * The backend already names the offending folder and the way out, so that text
 * is preferred; the fallback covers storage errors raised below the layer that
 * stamps the token.
 */
export function workspaceProblemMessage(error: unknown): string {
  const raw = rawInvokeError(error);
  const at = raw.toLowerCase().indexOf(WORKSPACE_NOT_WRITABLE);
  if (at >= 0) {
    const detail = raw.slice(at + WORKSPACE_NOT_WRITABLE.length).replace(/^[:\s]+/, "");
    if (detail) return detail;
  }
  return (
    "Zest cannot save chats in this project folder. " +
    "Use Open to choose a folder you own, such as one under Documents."
  );
}

export function shouldOfferProviderReconnect(error: unknown): boolean {
  const message = rawInvokeError(error).toLowerCase();
  return (
    message.includes("needs connect again") ||
    message.includes("needs to be reconnected") ||
    message.includes("auth_unavailable")
  );
}
