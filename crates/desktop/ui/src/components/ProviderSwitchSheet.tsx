import { useEffect, useRef, useState } from "react";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { XIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import { ProviderIcon } from "@/components/ProviderIcon";
import { getBackend } from "@/lib/backend";
import { isChatgptCodexRow } from "@/lib/chatgptCodex";
import { rawInvokeError } from "@/lib/invokeErrors";
import { recentVerifyFailed } from "@/lib/providerVerify";
import type { ProviderRow } from "@/lib/types";
import { cn } from "@/lib/utils";
import { ApiProviderForm } from "@/components/ApiProviderForm";
import { useDialogFocusTrap } from "@/lib/useDialogFocusTrap";

type Props = {
  open: boolean;
  providers: ProviderRow[];
  currentProviderId: string;
  busy: boolean;
  onClose: () => void;
  onSelect: (providerId: string) => void;
  onConnect: (providerId: string) => void;
  onRefresh: () => Promise<void>;
};

/** Vendor CLIs that own their own auth session and can be added as a
 *  first-class parent provider with no more input than an id and a model. */
const PARENT_CLI_DEFAULTS: Record<
  string,
  { label: string; model: string; configure: (id: string, model: string) => Promise<void> }
> = {
  claude: {
    label: "Enable Claude Code",
    model: "sonnet",
    configure: (id, model) => getBackend().configureClaudeCodeProvider({ id, model }),
  },
  codex: {
    label: "Enable Codex CLI",
    model: "gpt-5.6-sol",
    configure: (id, model) => getBackend().configureCodexCliProvider({ id, model }),
  },
  cursor: {
    label: "Enable Cursor CLI",
    model: "composer-2.5",
    configure: (id, model) => getBackend().configureCursorProvider({ id, model }),
  },
};

function statusLabel(row: ProviderRow): string {
  if (row.statusKind === "ready" && recentVerifyFailed(row.id)) {
    return "Reconnect required";
  }
  if (row.statusKind === "ready") return row.method || "Ready";
  if (row.statusKind === "unknown") return "Connection status unavailable";
  return row.detail || "Not ready";
}

/**
 * In-chat provider switch — WebView-safe positioned panel (no portals).
 */
export function ProviderSwitchSheet({
  open,
  providers,
  currentProviderId,
  busy,
  onClose,
  onSelect,
  onConnect,
  onRefresh,
}: Props) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const [addingApiProvider, setAddingApiProvider] = useState(false);
  const [keyProviderId, setKeyProviderId] = useState<string | null>(null);
  const [apiKey, setApiKey] = useState("");
  const [savingKey, setSavingKey] = useState(false);
  const [keyError, setKeyError] = useState<string | null>(null);
  const [enablingParentId, setEnablingParentId] = useState<string | null>(null);
  const [enableParentError, setEnableParentError] = useState<string | null>(null);
  const [cliAvailable, setCliAvailable] = useState(true);
  const [confirmChatgptId, setConfirmChatgptId] = useState<string | null>(null);
  useDialogFocusTrap(open, dialogRef);
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    void getBackend()
      .codexCliAvailable()
      .then((available) => {
        if (!cancelled) setCliAvailable(available);
      })
      .catch(() => {
        if (!cancelled) setCliAvailable(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open]);
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [busy, onClose, open]);

  if (!open) return null;

  return (
    <div className="absolute inset-0 z-40 flex items-end justify-center bg-black/45 p-4 sm:items-center">
      <button
        type="button"
        aria-label="Dismiss"
        className="absolute inset-0 cursor-default"
        disabled={busy}
        onClick={() => {
          if (!busy) onClose();
        }}
      />
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-label="Change provider"
        tabIndex={-1}
        className="relative z-10 w-full max-w-md overflow-hidden rounded-xl border border-border bg-popover text-popover-foreground shadow-2xl"
      >
        <div className="flex items-center justify-between border-b border-border/60 px-4 py-3">
          <div>
            <div className="text-sm font-semibold">Change provider</div>
            <div className="text-[11px] text-muted-foreground">
              {providers.some((row) => row.ownsAgentLoop)
                ? "API and ChatGPT chats stay here. A CLI switch opens a copy."
                : "This chat stays on the same thread."}
            </div>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            title="Close"
            disabled={busy}
            onClick={onClose}
          >
            <XIcon />
          </Button>
        </div>
        <ul className="m-0 max-h-[50vh] list-none overflow-y-auto p-2">
          {providers.map((row) => {
            const current = row.id === currentProviderId;
            const failed = recentVerifyFailed(row.id);
            const selectable =
              row.selectable &&
              (row.statusKind === "ready" || row.statusKind === "unknown") &&
              !failed;
            // A failed active API-key provider must remain recoverable from
            // this sheet. It is the one case where the current row needs an
            // action instead of a passive "Current" label.
            const keyConfigurable = row.method === "API key" && (current || row.statusKind === "unconfigured");
            return (
              <li key={row.id}>
                <div
                  className={cn(
                    "flex items-center gap-2 rounded-lg px-2.5 py-2",
                    current && "bg-secondary/60"
                  )}
                >
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2 truncate text-sm font-medium">
                      <ProviderIcon providerId={row.id} />
                      {row.label}
                    </div>
                    <div className="truncate text-[11px] text-muted-foreground">
                      {statusLabel(row)}
                    </div>
                  </div>
                  {current && !keyConfigurable ? (
                    <span className="shrink-0 text-[11px] text-muted-foreground">
                      Current
                    </span>
                  ) : selectable && !current ? (
                    <Button
                      type="button"
                      size="sm"
                      disabled={busy}
                      onClick={() => onSelect(row.id)}
                    >
                      Switch
                    </Button>
                  ) : keyConfigurable ? (
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      disabled={busy}
                      onClick={() => {
                        setKeyProviderId(row.id);
                        setApiKey("");
                        setKeyError(null);
                      }}
                    >
                      {current && row.statusKind === "ready" ? "Replace key" : "Set key"}
                    </Button>
                  ) : PARENT_CLI_DEFAULTS[row.id] &&
                    (row.id !== "codex" || cliAvailable) ? (
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      disabled={busy || enablingParentId === row.id}
                      onClick={() => {
                        const preset = PARENT_CLI_DEFAULTS[row.id];
                        setEnablingParentId(row.id);
                        setEnableParentError(null);
                        void preset
                          .configure(row.id, preset.model)
                          .then(async () => {
                            await onRefresh();
                            onSelect(row.id);
                          })
                          .catch((error) =>
                            setEnableParentError(
                              rawInvokeError(error) || `Could not enable ${row.label}. Try again.`
                            )
                          )
                          .finally(() => setEnablingParentId(null));
                      }}
                    >
                      {enablingParentId === row.id ? "Enabling…" : PARENT_CLI_DEFAULTS[row.id].label}
                    </Button>
                  ) : row.canConnect ? (
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      disabled={busy}
                      onClick={() => {
                        if (isChatgptCodexRow(row)) {
                          setConfirmChatgptId(row.id);
                          return;
                        }
                        onConnect(row.id);
                      }}
                    >
                      Connect
                    </Button>
                  ) : (
                    <span className="shrink-0 text-[11px] text-muted-foreground">
                      Configure in Settings
                    </span>
                  )}
                </div>
              </li>
            );
          })}
        </ul>
        {enableParentError ? (
          <p className="border-t border-border/60 px-3 py-2 text-[11px] text-destructive">
            {enableParentError}
          </p>
        ) : null}
        {keyProviderId ? (
          <form
            className="border-t border-border/60 p-3"
            onSubmit={(event) => {
              event.preventDefault();
              setSavingKey(true);
              setKeyError(null);
              void getBackend()
                .setProviderKey(keyProviderId, apiKey)
                .then(async () => {
                  setApiKey("");
                  await onRefresh();
                  setKeyProviderId(null);
                  onSelect(keyProviderId);
                })
                .catch(() => setKeyError("Could not save the API key. Try again."))
                .finally(() => setSavingKey(false));
            }}
          >
            <div className="text-xs font-medium">
              {providers.find((row) => row.id === keyProviderId)?.label ?? "Provider"} API key
            </div>
            <p className="mt-1 text-[11px] text-muted-foreground">
              Your key is stored securely by your operating system.
            </p>
            <div className="mt-2 flex gap-1.5">
              <input
                type="password"
                value={apiKey}
                onChange={(event) => setApiKey(event.target.value)}
                placeholder="Paste API key"
                autoComplete="off"
                className="min-w-0 flex-1 rounded-md border border-border/80 bg-background px-2.5 py-1.5 text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
                autoFocus
              />
              <Button type="button" size="sm" variant="ghost" disabled={savingKey} onClick={() => setKeyProviderId(null)}>
                Cancel
              </Button>
              <Button type="submit" size="sm" disabled={!apiKey.trim() || savingKey}>
                {savingKey ? "Saving…" : "Save"}
              </Button>
            </div>
            {keyError ? <p className="mt-2 text-xs text-destructive">{keyError}</p> : null}
          </form>
        ) : !addingApiProvider ? (
          <div className="border-t border-border/60 p-2">
            <Button type="button" variant="outline" className="w-full" disabled={busy} onClick={() => setAddingApiProvider(true)}>
              Add API provider
            </Button>
          </div>
        ) : (
          <ApiProviderForm
            onCancel={() => setAddingApiProvider(false)}
            onDone={async (id) => {
              await onRefresh();
              setAddingApiProvider(false);
              onSelect(id);
            }}
          />
        )}
      </div>
      <ConfirmDialog
        open={confirmChatgptId != null}
        title="Use ChatGPT for Codex"
        description="This signs you into ChatGPT and uses your Codex subscription from Zest. OpenAI does not publish this path for third-party apps, so it can stop working or affect your ChatGPT account. Continue only if you accept that."
        confirmLabel="Continue"
        cancelLabel="Cancel"
        onCancel={() => setConfirmChatgptId(null)}
        onConfirm={() => {
          const id = confirmChatgptId;
          setConfirmChatgptId(null);
          if (id) onConnect(id);
        }}
      />
    </div>
  );
}
