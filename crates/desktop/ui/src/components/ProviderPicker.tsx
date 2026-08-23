import { useEffect, useState } from "react";
import { CheckIcon, FolderOpenIcon, PlusIcon } from "lucide-react";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ApiProviderForm } from "@/components/ApiProviderForm";
import { AuthShell } from "@/components/AuthShell";
import { ProviderIcon } from "@/components/ProviderIcon";
import { BrandMark } from "@/components/BrandMark";
import { Button } from "@/components/ui/button";
import { isChatgptCodexRow } from "@/lib/chatgptCodex";
import { rawInvokeError } from "@/lib/invokeErrors";
import { recentVerifyFailed } from "@/lib/providerVerify";
import { getBackend } from "@/lib/backend";
import { cn } from "@/lib/utils";
import type { ProviderRow } from "@/lib/types";

type Props = {
  providers: ProviderRow[];
  selectedId: string | null;
  workspacePath: string | null;
  error: { message: string; workspace: boolean } | null;
  onSelect: (id: string) => void;
  onContinue: () => void;
  onConnect: () => void;
  onOpenFolder: () => void;
  onRefresh: () => Promise<void>;
  continuing: boolean;
  connecting: boolean;
};

/** Vendor CLIs that own their own auth session and can be added as a
 *  first-class parent provider with no more input than an id and a model. */
const PARENT_CLI_DEFAULTS: Record<
  string,
  {
    title: string;
    body: string;
    label: string;
    model: string;
    configure: (id: string, model: string) => Promise<void>;
  }
> = {
  claude: {
    title: "Use Claude Code subscription",
    body: "Zest will use your Claude Code CLI session as the parent agent, with no delegated worker involved.",
    label: "Enable Claude Code",
    model: "sonnet",
    configure: (id, model) => getBackend().configureClaudeCodeProvider({ id, model }),
  },
  codex: {
    title: "Use Codex CLI subscription",
    body: "Zest will use your Codex CLI session as the parent agent, with no delegated worker involved.",
    label: "Enable Codex CLI",
    model: "gpt-5.6-sol",
    configure: (id, model) => getBackend().configureCodexCliProvider({ id, model }),
  },
};

function shortRoot(root: string): string {
  const cleaned = root.replace(/^\\\\\?\\UNC\\/i, "\\\\").replace(/^\\\\\?\\/, "");
  const normalized = cleaned.replace(/\\/g, "/");
  const parts = normalized.split("/").filter(Boolean);
  if (parts.length <= 2) return cleaned;
  return parts.slice(-2).join("/");
}

export function ProviderPicker({
  providers,
  selectedId,
  workspacePath,
  error,
  onSelect,
  onContinue,
  onConnect,
  onOpenFolder,
  onRefresh,
  continuing,
  connecting,
}: Props) {
  const [apiKey, setApiKey] = useState("");
  const [refreshing, setRefreshing] = useState(false);
  const [savingKey, setSavingKey] = useState(false);
  const [keyError, setKeyError] = useState<string | null>(null);
  const [addingApiProvider, setAddingApiProvider] = useState(false);
  const [configuringParentId, setConfiguringParentId] = useState<string | null>(null);
  const [configureParentError, setConfigureParentError] = useState<string | null>(null);
  const [cliAvailable, setCliAvailable] = useState<boolean | null>(null);
  const [confirmChatgpt, setConfirmChatgpt] = useState(false);

  useEffect(() => {
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
  }, []);
  const selected = providers.find((p) => p.id === selectedId) ?? null;
  const selectedNeedsConnect =
    selected != null && recentVerifyFailed(selected.id);
  const ready =
    selected?.selectable === true &&
    !selectedNeedsConnect &&
    (selected?.statusKind === "ready" || selected?.statusKind === "unknown");

  async function retryProviderList() {
    if (refreshing) return;
    setRefreshing(true);
    try {
      await onRefresh();
    } finally {
      setRefreshing(false);
    }
  }

  return (
    <AuthShell>
      <header className="mb-6">
        <div className="mb-4">
          <BrandMark />
        </div>
        <h1 className="m-0 mb-1.5 text-[22px] font-semibold leading-tight tracking-[-0.4px]">
          Choose a provider
        </h1>
        <p className="m-0 max-w-[38ch] text-[13px] leading-relaxed text-muted-foreground">
          Use an existing sign-in, or connect a provider with your own API key.
          Zest never asks for your password.
        </p>
      </header>

      <div className="mb-5 flex items-center gap-2 border-b border-border/60 pb-4">
        <div className="min-w-0 flex-1">
          <div className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
            Project folder (optional)
          </div>
          <div
            className="mt-0.5 truncate font-mono text-xs text-foreground/85"
            title={workspacePath ?? undefined}
          >
            {workspacePath ? shortRoot(workspacePath) : "No project selected"}
          </div>
        </div>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={continuing || connecting}
          onClick={onOpenFolder}
        >
          <FolderOpenIcon className="size-3.5" />
          Open
        </Button>
      </div>

      {/*
        A folder problem belongs against the folder, not under the provider
        list. As a bare red line beneath the Codex row it read as "Codex is
        broken" — while the actual remedy was the Open button above, which
        nobody had reason to connect to the failure.
      */}
      {error?.workspace ? (
        <div className="mb-5 rounded-lg border border-destructive/40 bg-destructive/10 p-3">
          <div className="text-xs font-medium text-destructive">
            This folder cannot be used as a project
          </div>
          <p className="mt-1 text-[11px] leading-relaxed text-foreground/80">{error.message}</p>
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="mt-2.5 w-full"
            disabled={continuing || connecting}
            onClick={onOpenFolder}
          >
            <FolderOpenIcon className="size-3.5" />
            Choose a different folder
          </Button>
        </div>
      ) : null}

      <div className="mb-2 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
        Available on this machine
      </div>

      <ul
        className="m-0 list-none overflow-hidden rounded-lg border border-border/70 bg-card/40 p-0"
        role="listbox"
        aria-label="Providers"
      >
        {providers.map((p, index) => {
          const selectedRow = p.id === selectedId;
          const verifyFailed = recentVerifyFailed(p.id);
          const usable = p.selectable && !verifyFailed;
          const detail = verifyFailed
            ? "Connection check failed — reconnect"
            : p.statusKind === "ready"
              ? usable
                ? p.method
                : p.detail
              : p.statusKind === "unknown"
                ? shortenUnknown(p.detail)
                : p.detail;
          const statusLabel = verifyFailed
            ? "Reconnect"
            : p.statusKind === "ready" && !p.selectable
              ? "Configure"
              : p.statusLabel;

          return (
            <li
              key={p.id}
              className="animate-in fade-in slide-in-from-bottom-1 fill-mode-both duration-200"
              style={{ animationDelay: `${40 + index * 35}ms` }}
            >
              <button
                type="button"
                role="option"
                aria-selected={selectedRow}
                onClick={() => onSelect(p.id)}
                className={cn(
                  "grid w-full cursor-pointer grid-cols-[10px_1fr_auto] items-center gap-3 px-3.5 py-3 text-left font-inherit outline-none transition-[background-color,color] duration-150",
                  "border-b border-border/50 last:border-b-0",
                  "hover:bg-accent/50 focus-visible:bg-accent/50 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/40",
                  selectedRow && "bg-accent/70"
                )}
              >
                <span
                  className={cn(
                    "justify-self-center size-2 rounded-full transition-colors duration-150",
                    usable && p.statusKind === "ready" && "bg-primary",
                    verifyFailed && "bg-amber-400",
                    usable && p.statusKind === "unknown" && "bg-[#c4c4c4]",
                    !verifyFailed &&
                      (p.statusKind === "not_logged_in" ||
                        p.statusKind === "unconfigured") &&
                      "bg-transparent shadow-[inset_0_0_0_1.5px_var(--muted-foreground)]"
                  )}
                  aria-hidden
                />
                <span className="min-w-0">
                  <div className="flex items-center gap-2 text-[13px] font-medium tracking-[-0.1px]">
                    {/* Decorative — the name it identifies is right beside it. */}
                    <ProviderIcon providerId={p.id} />
                    {p.label}
                    {selectedRow ? (
                      <CheckIcon
                        className="size-3 text-primary"
                        strokeWidth={2.5}
                        aria-hidden
                      />
                    ) : null}
                  </div>
                  <div className="mt-0.5 truncate text-[11px] text-muted-foreground">{detail}</div>
                </span>
                <span
                  className={cn(
                    "whitespace-nowrap text-[11px] font-medium text-muted-foreground",
                    usable && p.statusKind === "ready" && "text-primary",
                    verifyFailed && "text-amber-400"
                  )}
                >
                  {statusLabel}
                </span>
              </button>
            </li>
          );
        })}
      </ul>

      {error && !error.workspace ? (
        <div className="mt-3 rounded-lg border border-destructive/40 bg-destructive/10 p-3">
          <p className="m-0 text-xs leading-relaxed text-destructive">{error.message}</p>
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="mt-2.5 w-full"
            disabled={continuing || connecting || refreshing}
            onClick={() => void retryProviderList()}
          >
            {refreshing ? "Checking…" : "Try again"}
          </Button>
        </div>
      ) : null}

      {selected &&
      !selected.configured &&
      PARENT_CLI_DEFAULTS[selected.id] &&
      (selected.id !== "codex" || cliAvailable === true) ? (
        <div className="mt-4 rounded-lg border border-border/70 bg-card/40 p-3">
          <div className="text-xs font-medium">{PARENT_CLI_DEFAULTS[selected.id].title}</div>
          <p className="mt-1 text-[11px] leading-relaxed text-muted-foreground">
            {PARENT_CLI_DEFAULTS[selected.id].body}
          </p>
          <Button
            type="button"
            size="sm"
            className="mt-2.5 w-full"
            disabled={continuing || connecting || configuringParentId === selected.id}
            onClick={() => {
              const id = selected.id;
              const preset = PARENT_CLI_DEFAULTS[id];
              setConfiguringParentId(id);
              setConfigureParentError(null);
              void preset
                .configure(id, preset.model)
                .then(async () => {
                  await onRefresh();
                  onSelect(id);
                })
                .catch((error) =>
                  setConfigureParentError(
                    rawInvokeError(error) || `Could not enable ${preset.label}. Try again.`
                  )
                )
                .finally(() => setConfiguringParentId(null));
            }}
          >
            {configuringParentId === selected.id ? "Enabling…" : PARENT_CLI_DEFAULTS[selected.id].label}
          </Button>
          {configureParentError ? (
            <p className="mt-2 text-xs text-destructive">{configureParentError}</p>
          ) : null}
        </div>
      ) : null}

      {/*
        Without this the picker is a dead end for anyone who has no CLI sign-in:
        adding a key-based provider was only reachable from the in-chat "Change
        provider" sheet, which needs a working provider to reach. Someone whose
        Codex account is refused could not get in at all.
      */}
      {addingApiProvider ? (
        <div className="mt-4 overflow-hidden rounded-lg border border-border/70">
          <ApiProviderForm
            onCancel={() => setAddingApiProvider(false)}
            onDone={async (id) => {
              await onRefresh();
              setAddingApiProvider(false);
              onSelect(id);
            }}
          />
        </div>
      ) : (
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="mt-3 w-full"
          disabled={continuing || connecting}
          onClick={() => setAddingApiProvider(true)}
        >
          <PlusIcon className="size-3.5" />
          Add a provider with an API key
        </Button>
      )}

      {selected?.method === "API key" ? (
        <div className="mt-4 rounded-lg border border-border/70 bg-card/40 p-3">
          <div className="text-xs font-medium">{selected.label} API key</div>
          <p className="mt-1 text-[11px] leading-relaxed text-muted-foreground">
            Your key is stored securely by your operating system.
          </p>
          <div className="mt-2 flex gap-1.5">
            <input
              type="password"
              value={apiKey}
              onChange={(event) => setApiKey(event.target.value)}
              placeholder={selected.statusKind === "ready" ? "Replace key" : "Paste API key"}
              autoComplete="off"
              className="min-w-0 flex-1 rounded-md border border-border/80 bg-background px-2.5 py-1.5 text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
            />
            <Button
              type="button"
              size="sm"
              disabled={!apiKey.trim() || savingKey || connecting}
              onClick={() => {
                setSavingKey(true);
                setKeyError(null);
                void getBackend()
                  .setProviderKey(selected.id, apiKey)
                  .then(() => {
                    setApiKey("");
                    return onRefresh();
                  })
                  .catch(() => setKeyError("Could not save the API key. Try again."))
                  .finally(() => setSavingKey(false));
              }}
            >
              {savingKey ? "Saving…" : "Save"}
            </Button>
          </div>
          {keyError ? <p className="mt-2 text-xs text-destructive">{keyError}</p> : null}
        </div>
      ) : null}

      <footer className="mt-6 flex justify-end gap-2">
        {selected?.canConnect ? (
          <Button
            type="button"
            variant="outline"
            disabled={continuing || connecting}
            onClick={() => {
              if (selected && isChatgptCodexRow(selected)) {
                setConfirmChatgpt(true);
                return;
              }
              onConnect();
            }}
          >
            {connecting
              ? "Connecting…"
              : selected.statusKind === "ready" || selectedNeedsConnect
                ? "Reconnect"
                : "Connect"}
          </Button>
        ) : null}
        <Button
          type="button"
          disabled={!ready || continuing || connecting}
          onClick={onContinue}
        >
          {continuing ? "Starting…" : "Continue"}
        </Button>
      </footer>
      <ConfirmDialog
        open={confirmChatgpt}
        title="Use ChatGPT for Codex"
        description="This signs you into ChatGPT and uses your Codex subscription from Zest. OpenAI does not publish this path for third-party apps, so it can stop working or affect your ChatGPT account. Continue only if you accept that."
        confirmLabel="Continue"
        cancelLabel="Cancel"
        onCancel={() => setConfirmChatgpt(false)}
        onConfirm={() => {
          setConfirmChatgpt(false);
          onConnect();
        }}
      />
    </AuthShell>
  );
}

function shortenUnknown(_detail: string) {
  return "Connection status unavailable";
}
