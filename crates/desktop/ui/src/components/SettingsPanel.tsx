import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import {
  BotIcon,
  BookOpenIcon,
  ChartColumnIcon,
  ChevronRightIcon,
  FolderOpenIcon,
  KeyboardIcon,
  type LucideIcon,
  PuzzleIcon,
  RefreshCwIcon,
  ScrollTextIcon,
  ServerIcon,
  TypeIcon,
  UserIcon,
  XIcon,
} from "lucide-react";

import { FontPicker } from "@/components/FontPicker";
import {
  KeyboardShortcuts,
  useScrollIntoViewOnBump,
} from "@/components/KeyboardShortcuts";
import { NowPlayingCard } from "@/components/NowPlayingCard";
import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { WorkerModelPicker } from "@/components/WorkerModelPicker";
import { getBackend, type SkillSummary } from "@/lib/backend";
import { chipLabel, effortsForModel, modelLabel, type EffortId } from "@/lib/models";
import { optimizeAvatarFile } from "@/lib/optimizeAvatar";
import { useDialogFocusTrap } from "@/lib/useDialogFocusTrap";
import type {
  ExternalAgentCheck,
  ExternalAgentRow,
  NowPlayingView,
  PluginView,
  ProviderRow,
  SessionInfo,
  UsageSnapshot,
  UserProfile,
} from "@/lib/types";
import { cn } from "@/lib/utils";

type Props = {
  open: boolean;
  session: SessionInfo;
  model: string;
  effort: EffortId;
  sending: boolean;
  profile: UserProfile;
  /** Bumped to open and scroll to the Keyboard shortcuts section. */
  focusShortcuts?: number;
  /** Open the User section first (avatar click). */
  focusUser?: boolean;
  onClose: () => void;
  onChangeProvider: () => void;
  /** Rebuild the session after credentials or an external worker change. */
  onReloadSession?: () => Promise<void>;
  onReconnect: () => void;
  onProviderKeyRemoved?: (providerId: string) => void;
  onOpenFolder: () => void;
  onProfileChange: (profile: UserProfile) => void;
};

const CUSTOM_SOFT_LIMIT = 8000;

function formatAge(secs: number) {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  return `${Math.floor(secs / 3600)}h`;
}

function externalTokenSummary(row: UsageSnapshot["externalWorkers"][number]) {
  if (row.reportedTokenTotal == null) return "Tokens not reported";
  const coverage =
    row.tokenReports < row.invocations
      ? ` · ${row.tokenReports}/${row.invocations} runs reported`
      : "";
  return `${row.reportedTokenTotal.toLocaleString()} tokens reported${coverage}`;
}

function externalContextSummary(row: UsageSnapshot["externalWorkers"][number]) {
  if (row.contextUsed == null && row.contextSize == null) return "Context not reported";
  if (row.contextUsed != null && row.contextSize != null) {
    return `Context ${row.contextUsed.toLocaleString()} / ${row.contextSize.toLocaleString()}`;
  }
  const value = row.contextUsed ?? row.contextSize;
  return value == null ? "Context not reported" : `Context ${value.toLocaleString()} reported`;
}

function SettingsSection({
  title,
  hint,
  icon: Icon,
  defaultOpen = false,
  openSignal = 0,
  children,
}: {
  title: string;
  hint?: string;
  icon: LucideIcon;
  defaultOpen?: boolean;
  /**
   * Incrementing counter that forces the section open.
   *
   * A boolean cannot express "open it *again*": once `defaultOpen` has gone
   * true it never changes, so a second request to jump here would silently do
   * nothing if the user had collapsed the section in between.
   */
  openSignal?: number;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(defaultOpen);

  useEffect(() => {
    if (defaultOpen) setOpen(true);
  }, [defaultOpen]);

  useEffect(() => {
    if (openSignal > 0) setOpen(true);
  }, [openSignal]);

  return (
    <Collapsible open={open} onOpenChange={setOpen} className="border-b border-border/50">
      <CollapsibleTrigger
        className={cn(
          "flex w-full cursor-pointer items-center gap-2.5 px-4 py-3 text-left outline-none transition-colors",
          "hover:bg-accent/40 focus-visible:ring-2 focus-visible:ring-ring/40"
        )}
      >
        <ChevronRightIcon
          className={cn(
            "size-3.5 shrink-0 text-muted-foreground transition-transform duration-150",
            open && "rotate-90"
          )}
        />
        <Icon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
        <span className="min-w-0 flex-1">
          <span className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
            {title}
          </span>
          {hint && !open ? (
            <span className="mt-0.5 block truncate text-[11px] text-muted-foreground/80">
              {hint}
            </span>
          ) : null}
        </span>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="px-4 pb-4 pt-3">{children}</div>
      </CollapsibleContent>
    </Collapsible>
  );
}

/** Plain overlay panel; portal-based menus are avoided for webview stability. */
export function SettingsPanel({
  open,
  session,
  model,
  effort,
  sending,
  profile,
  focusUser = false,
  focusShortcuts = 0,
  onClose,
  onChangeProvider,
  onReloadSession,
  onReconnect,
  onProviderKeyRemoved,
  onOpenFolder,
  onProfileChange,
}: Props) {
  const supportsEffort = effortsForModel(session.models, model).length > 0;
  const panelRef = useRef<HTMLDivElement>(null);
  const shortcutsRef = useScrollIntoViewOnBump(focusShortcuts);
  const titleId = useId();
  useDialogFocusTrap(open, panelRef);
  const [provider, setProvider] = useState<ProviderRow | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [providerKey, setProviderKey] = useState("");
  const [providerKeyPresent, setProviderKeyPresent] = useState(false);
  const [providerKeySaving, setProviderKeySaving] = useState(false);

  const [externalAgents, setExternalAgents] = useState<ExternalAgentRow[]>([]);
  const [externalChecks, setExternalChecks] = useState<Record<string, ExternalAgentCheck>>({});
  const [externalBusy, setExternalBusy] = useState<{
    id: string;
    action: "saving" | "checking" | "mcp" | "model";
  } | null>(null);
  const [externalLoading, setExternalLoading] = useState(false);
  const [externalError, setExternalError] = useState<string | null>(null);

  const [customPrompt, setCustomPrompt] = useState("");
  const [savedCustom, setSavedCustom] = useState("");
  const [basePrompt, setBasePrompt] = useState("");
  const [promptPath, setPromptPath] = useState(".zest/system.md");
  const [promptSaving, setPromptSaving] = useState(false);
  const [promptError, setPromptError] = useState<string | null>(null);
  const [promptSavedFlash, setPromptSavedFlash] = useState(false);

  const [skills, setSkills] = useState<SkillSummary[]>([]);
  const [usage, setUsage] = useState<UsageSnapshot | null>(null);
  const [plugins, setPlugins] = useState<PluginView[]>([]);
  const [nowPlaying, setNowPlaying] = useState<NowPlayingView | null>(null);
  const [pluginBusy, setPluginBusy] = useState<string | null>(null);
  const [pluginFolderBusy, setPluginFolderBusy] = useState(false);
  const [displayName, setDisplayName] = useState(profile.displayName);
  const [avatarDataUrl, setAvatarDataUrl] = useState(profile.avatarDataUrl);
  const [profileSaving, setProfileSaving] = useState(false);
  const [profileError, setProfileError] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setDisplayName(profile.displayName);
    setAvatarDataUrl(profile.avatarDataUrl);
  }, [open, profile.displayName, profile.avatarDataUrl]);

  useEffect(() => {
    if (!open) return;

    let cancelled = false;
    setLoading(true);
    setError(null);
    setPromptError(null);
    setProfileError(null);
    setExternalLoading(true);
    setExternalError(null);
    setExternalChecks({});

    const backend = getBackend();
    // Settled, not all: these are independent sections, and one of them
    // failing used to blank the other three. The system prompt in particular
    // needs the live session, which is unavailable while a turn streams —
    // that must not take Usage and Skills down with it.
    Promise.allSettled([
      backend.listProviders(),
      backend.listExternalAgents(),
      backend.getSystemPrompt(),
      backend.listSkills(),
      backend.usageSnapshot(),
      backend.listPlugins(),
      backend.nowPlaying(),
    ])
      .then(([rowsR, externalR, promptR, skillsR, snapR, pluginsR, nowPlayingR]) => {
        if (cancelled) return;

        if (rowsR.status === "fulfilled") {
          const current = rowsR.value.find((p) => p.id === session.provider) ?? null;
          setProvider(current);
          if (current?.method === "API key") {
            void backend.providerKeyPresent(current.id).then(setProviderKeyPresent).catch(() => setProviderKeyPresent(false));
          }
        } else {
          setError("Could not load provider settings. Try again.");
        }

        if (externalR.status === "fulfilled") {
          setExternalAgents(externalR.value);
        } else {
          setExternalAgents([]);
          setExternalError("Could not load external workers. Try again.");
        }

        if (promptR.status === "fulfilled") {
          setCustomPrompt(promptR.value.custom);
          setSavedCustom(promptR.value.custom);
          setBasePrompt(promptR.value.base);
          setPromptPath(promptR.value.customPath);
        } else {
          setBasePrompt("");
          setPromptError("Could not load your instructions. Try again.");
        }

        setSkills(skillsR.status === "fulfilled" ? skillsR.value : []);
        if (skillsR.status === "rejected") {
          setError("Could not load skills. Try again.");
        }

        setUsage(snapR.status === "fulfilled" ? snapR.value : null);
        setPlugins(pluginsR.status === "fulfilled" ? pluginsR.value : []);
        setNowPlaying(nowPlayingR.status === "fulfilled" ? nowPlayingR.value : null);
      })
      .finally(() => {
        if (!cancelled) {
          setLoading(false);
          setExternalLoading(false);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [open, session.provider]);

  // Refresh usage after terminal turns without resetting prompt drafts.
  useEffect(() => {
    if (!open || sending) return;
    let cancelled = false;
    getBackend()
      .usageSnapshot()
      .then((snap) => {
        if (!cancelled) setUsage(snap);
      })
      .catch(() => {
        /* keep last good snapshot */
      });
    return () => {
      cancelled = true;
    };
  }, [open, sending]);

  // Media metadata is intentionally polled only while Settings is visible and
  // the user has opted into the plugin. No background listener is kept alive.
  useEffect(() => {
    if (!open || !plugins.some((plugin) => plugin.id === "now-playing" && plugin.enabled)) {
      return;
    }
    const timer = window.setInterval(() => {
      void getBackend()
        .nowPlaying()
        .then(setNowPlaying)
        .catch(() => undefined);
    }, 5_000);
    return () => window.clearInterval(timer);
  }, [open, plugins]);

  useEffect(() => {
    if (!open) return;

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };

    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [open, onClose]);

  useEffect(() => {
    if (!open) return;
    panelRef.current?.focus();
  }, [open]);

  if (!open) return null;

  const canConnect = provider?.canConnect ?? false;
  const connectLabel =
    provider?.statusKind === "ready" || provider?.statusKind === "unknown"
      ? "Reconnect"
      : "Connect";

  const promptDirty = customPrompt !== savedCustom;
  const overSoftLimit = customPrompt.length > CUSTOM_SOFT_LIMIT;
  const promptHint = savedCustom.trim()
    ? `${savedCustom.trim().slice(0, 42)}${savedCustom.trim().length > 42 ? "…" : ""}`
    : "Default Zest rules";

  async function savePrompt() {
    setPromptSaving(true);
    setPromptError(null);
    setPromptSavedFlash(false);
    try {
      const info = await getBackend().setSystemPrompt(customPrompt);
      setCustomPrompt(info.custom);
      setSavedCustom(info.custom);
      setBasePrompt(info.base);
      setPromptPath(info.customPath);
      setPromptSavedFlash(true);
      window.setTimeout(() => setPromptSavedFlash(false), 1600);
      const nextSkills = await getBackend().listSkills().catch(() => skills);
      setSkills(nextSkills);
    } catch {
      setPromptError("Could not save your instructions. Try again.");
    } finally {
      setPromptSaving(false);
    }
  }

  function revertPrompt() {
    setCustomPrompt(savedCustom);
    setPromptError(null);
  }

  const profileDirty =
    displayName !== profile.displayName || avatarDataUrl !== profile.avatarDataUrl;

  async function saveProfile() {
    setProfileSaving(true);
    setProfileError(null);
    try {
      const next = await getBackend().setUserProfile({
        displayName: displayName.trim(),
        avatarDataUrl,
      });
      onProfileChange(next);
      setDisplayName(next.displayName);
      setAvatarDataUrl(next.avatarDataUrl);
    } catch {
      setProfileError("Could not save your profile. Try again.");
    } finally {
      setProfileSaving(false);
    }
  }

  async function saveProviderKey() {
    if (!provider || !providerKey.trim()) return;
    setProviderKeySaving(true);
    setError(null);
    try {
      await getBackend().setProviderKey(provider.id, providerKey);
      setProviderKey("");
      setProviderKeyPresent(true);
      const rows = await getBackend().listProviders();
      setProvider(rows.find((row) => row.id === provider.id) ?? provider);
      // The runtime captures the credential when the session is built. A
      // saved replacement therefore needs an explicit session rebuild before
      // the next turn can use it; this is still user-initiated by clicking
      // Save, never an automatic provider switch.
      if (onReloadSession) await onReloadSession();
    } catch {
      setError("Could not save the API key. Try again.");
    } finally {
      setProviderKeySaving(false);
    }
  }

  async function removeProviderKey() {
    if (!provider) return;
    setProviderKeySaving(true);
    setError(null);
    try {
      await getBackend().deleteProviderKey(provider.id);
      setProviderKeyPresent(false);
      onProviderKeyRemoved?.(provider.id);
    } catch {
      setError("Could not remove the API key. Try again.");
    } finally {
      setProviderKeySaving(false);
    }
  }

  async function toggleExternalAgent(agent: ExternalAgentRow) {
    if (!agent.preset || sending) return;
    setExternalBusy({ id: agent.id, action: "saving" });
    setExternalError(null);
    try {
      await getBackend().setExternalAgent(agent.id, !agent.configured);
      setExternalAgents(await getBackend().listExternalAgents());
      setExternalChecks((previous) => {
        const next = { ...previous };
        delete next[agent.id];
        return next;
      });
      if (onReloadSession) await onReloadSession();
    } catch {
      setExternalError(
        `Could not ${agent.configured ? "disable" : "enable"} ${agent.label}. Try again.`
      );
    } finally {
      setExternalBusy(null);
    }
  }

  async function togglePlugin(plugin: PluginView) {
    setPluginBusy(plugin.id);
    try {
      const next = await getBackend().setPluginEnabled(plugin.id, !plugin.enabled);
      setPlugins(next);
      setNowPlaying(await getBackend().nowPlaying());
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not change this extra.");
    } finally {
      setPluginBusy(null);
    }
  }

  async function refreshPlugins() {
    try {
      const backend = getBackend();
      const [next, music] = await Promise.all([backend.listPlugins(), backend.nowPlaying()]);
      setPlugins(next);
      setNowPlaying(music);
    } catch {
      setError("Could not refresh extras.");
    }
  }

  async function openPluginFolder() {
    setPluginFolderBusy(true);
    try {
      await getBackend().openPluginsFolder();
    } catch {
      setError("Could not open the extras folder.");
    } finally {
      setPluginFolderBusy(false);
    }
  }

  async function checkExternalAgent(agent: ExternalAgentRow) {
    if (!agent.configured || sending) return;
    setExternalBusy({ id: agent.id, action: "checking" });
    setExternalError(null);
    try {
      const result = await getBackend().checkExternalAgent(agent.id);
      setExternalChecks((previous) => ({ ...previous, [agent.id]: result }));
    } catch {
      setExternalError(`Could not check ${agent.label}. Try again.`);
    } finally {
      setExternalBusy(null);
    }
  }

  async function toggleExternalAgentMcp(agent: ExternalAgentRow) {
    if (!agent.preset || !agent.configured || sending) return;
    setExternalBusy({ id: agent.id, action: "mcp" });
    setExternalError(null);
    try {
      await getBackend().setExternalAgentMcp(agent.id, !agent.mcpAllowed);
      setExternalAgents(await getBackend().listExternalAgents());
      if (onReloadSession) await onReloadSession();
    } catch {
      setExternalError(`Could not update MCP access for ${agent.label}. Try again.`);
    } finally {
      setExternalBusy(null);
    }
  }

  async function setExternalAgentModel(agent: ExternalAgentRow, nextModel: string) {
    if (!agent.preset || !agent.configured || sending || nextModel === agent.model) return;
    setExternalBusy({ id: agent.id, action: "model" });
    setExternalError(null);
    try {
      await getBackend().setExternalAgentModel(agent.id, nextModel || null);
      setExternalAgents(await getBackend().listExternalAgents());
      if (onReloadSession) await onReloadSession();
    } catch {
      setExternalError(`Could not update the model for ${agent.label}. Try again.`);
    } finally {
      setExternalBusy(null);
    }
  }

  async function onPickAvatar(file: File | null) {
    if (!file) return;
    setProfileError(null);
    try {
      setAvatarDataUrl(await optimizeAvatarFile(file));
    } catch {
      setProfileError("Could not use that image. Choose a JPEG under 48 KB.");
    }
  }

  return (
    <div className="absolute inset-0 z-40 flex justify-end overflow-hidden">
      <button
        type="button"
        aria-label="Close settings"
        className="absolute inset-0 cursor-pointer bg-black/45 animate-in fade-in duration-150"
        onClick={onClose}
      />
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        className={cn(
          "relative flex h-full w-full max-w-[340px] shrink-0 flex-col border-l border-border bg-[var(--chat-header,#121314)] text-foreground shadow-2xl outline-none",
          "animate-in slide-in-from-right duration-200 ease-out"
        )}
      >
        <header className="flex shrink-0 items-center justify-between border-b border-border/60 px-4 py-3">
          <h2 id={titleId} className="text-sm font-semibold tracking-[-0.2px]">
            Settings
          </h2>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            title="Close"
            onClick={onClose}
          >
            <XIcon />
          </Button>
        </header>

        <div className="min-h-0 flex-1 overflow-y-auto">
          <SettingsSection
            title="User"
            icon={UserIcon}
            hint={displayName.trim() || "Name & photo"}
            defaultOpen={focusUser}
          >
            <div className="flex items-center gap-3">
              <button
                type="button"
                className="grid size-14 cursor-pointer place-items-center overflow-hidden rounded-xl bg-card ring-1 ring-border outline-none transition-opacity hover:opacity-90 focus-visible:ring-2 focus-visible:ring-ring/50"
                title="Change avatar"
                onClick={() => fileRef.current?.click()}
              >
                {avatarDataUrl ? (
                  <img src={avatarDataUrl} alt="" className="size-full object-cover" />
                ) : (
                  <span className="text-sm text-muted-foreground">PFP</span>
                )}
              </button>
              <input
                ref={fileRef}
                type="file"
                accept="image/png,image/jpeg,image/webp,image/gif"
                className="hidden"
                onChange={(e) => {
                  void onPickAvatar(e.target.files?.[0] ?? null);
                }}
              />
              <div className="min-w-0 flex-1">
                <label className="mb-1 block text-[11px] text-muted-foreground">
                  Display name
                </label>
                <input
                  value={displayName}
                  onChange={(e) => setDisplayName(e.target.value)}
                  disabled={sending || profileSaving}
                  placeholder="Your name"
                  className="w-full rounded-md border border-border/80 bg-card/80 px-2.5 py-1.5 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
                />
              </div>
            </div>
            {profileError ? (
              <p className="mt-2 text-xs text-destructive">{profileError}</p>
            ) : null}
            <div className="mt-2.5 flex flex-wrap gap-1.5">
              <Button
                type="button"
                size="sm"
                disabled={sending || profileSaving || !profileDirty}
                onClick={() => void saveProfile()}
              >
                {profileSaving ? "Saving…" : "Save profile"}
              </Button>
              {avatarDataUrl ? (
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={sending || profileSaving}
                  onClick={() => setAvatarDataUrl("")}
                >
                  Remove photo
                </Button>
              ) : null}
            </div>
          </SettingsSection>

          <SettingsSection
            title="Typography"
            icon={TypeIcon}
            hint="Font family & appearance"
          >
            <FontPicker />
          </SettingsSection>

          <SettingsSection
            title="Provider"
            icon={ServerIcon}
            hint={`${session.label} · ${provider?.statusLabel ?? session.provider}`}
          >
            <div className="rounded-lg border border-border/80 bg-card/80 px-3 py-2.5">
              <div className="text-sm font-medium">{session.label}</div>
              <div className="mt-0.5 text-[11px] text-muted-foreground">
                {provider?.statusLabel ?? session.provider}
              </div>
              <div
                className="mt-1 break-all font-mono text-[11px] text-muted-foreground"
                title={session.root}
              >
                {session.root}
              </div>
              <div className="mt-3 flex flex-wrap gap-1.5">
                <Button
                  type="button"
                  size="sm"
                  variant="secondary"
                  disabled={sending}
                  onClick={onOpenFolder}
                >
                  Change folder
                </Button>
                {canConnect ? (
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    disabled={sending}
                    onClick={onReconnect}
                  >
                    {connectLabel}
                  </Button>
                ) : null}
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={sending}
                  onClick={onChangeProvider}
                >
                  Change provider
                </Button>
                {provider?.method === "API key" ? (
                  <div className="mt-2 w-full">
                    <label className="mb-1 block text-[11px] text-muted-foreground">
                      API key {providerKeyPresent ? "configured" : "not configured"}
                    </label>
                    <div className="flex gap-1.5">
                      <input
                        type="password"
                        value={providerKey}
                        onChange={(event) => setProviderKey(event.target.value)}
                        placeholder={providerKeyPresent ? "Replace key" : "Paste API key"}
                        autoComplete="off"
                        className="min-w-0 flex-1 rounded-md border border-border/80 bg-background px-2 py-1.5 text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
                      />
                      <Button type="button" size="sm" disabled={!providerKey.trim() || providerKeySaving} onClick={() => void saveProviderKey()}>
                        {providerKeySaving ? "Saving…" : "Save"}
                      </Button>
                    </div>
                    {providerKeyPresent ? (
                      <Button type="button" size="sm" variant="ghost" disabled={providerKeySaving} onClick={() => void removeProviderKey()}>
                        Remove key
                      </Button>
                    ) : null}
                  </div>
                ) : null}
              </div>
            </div>
            <p className="mt-2 text-xs leading-relaxed text-muted-foreground">
              Using {supportsEffort ? chipLabel(model, effort) : modelLabel(model)}. Change model{supportsEffort ? " and effort" : ""} in the composer.
            </p>
          </SettingsSection>

          <SettingsSection
            title="CLI delegation"
            icon={BotIcon}
            hint={
              externalLoading
                ? "Loading..."
                : externalAgents.length === 0
                  ? "Unavailable"
                  : `${externalAgents.filter((agent) => agent.configured).length} enabled`
            }
          >
            <div
              aria-busy={externalLoading || externalBusy !== null}
              className="min-w-0"
            >
              <p className="mb-3 text-xs leading-relaxed text-muted-foreground">
                Enable a CLI worker to handle bounded tasks. This does not change your selected
                provider or route normal chats through the CLI. Sign in to the CLI first; MCP
                access is optional and controlled separately for each worker.
              </p>
              {externalLoading ? (
                <p className="text-xs text-muted-foreground" role="status">
                  Loading external workers...
                </p>
              ) : externalAgents.length ? (
                <ul className="m-0 flex list-none flex-col gap-2 p-0">
                {externalAgents.map((agent) => {
                  const check = externalChecks[agent.id];
                  const busy = externalBusy?.id === agent.id;
                  return (
                    <li
                      key={agent.id}
                      className="rounded-lg border border-border/80 bg-card/70 px-3 py-2.5"
                    >
                      <div className="flex items-start justify-between gap-2">
                        <div className="min-w-0">
                          <div className="truncate text-sm font-medium">{agent.label}</div>
                          <div className="mt-0.5 text-[10px] uppercase tracking-wide text-muted-foreground">
                            {agent.statusLabel}
                          </div>
                          <div className="mt-0.5 text-[11px] text-muted-foreground">
                            {agent.mode} {"·"} {agent.workspace}
                          </div>
                        </div>
                        {agent.preset ? (
                          <Button
                            type="button"
                            size="sm"
                            variant={agent.configured ? "secondary" : "outline"}
                            disabled={sending || busy || externalBusy !== null || externalLoading}
                            aria-pressed={agent.configured}
                            aria-label={`${agent.configured ? "Disable delegation through" : "Enable delegation through"} ${agent.label}`}
                            onClick={() => void toggleExternalAgent(agent)}
                          >
                            {busy && externalBusy?.action === "saving"
                              ? "Saving..."
                              : agent.configured
                                ? "Disable delegation"
                                : "Enable delegation"}
                          </Button>
                        ) : (
                          <span className="shrink-0 text-[10px] uppercase tracking-wide text-muted-foreground">
                            TOML
                          </span>
                        )}
                      </div>
                      <p className="mt-2 text-[11px] leading-relaxed text-muted-foreground">
                        {agent.detail}
                      </p>
                      {check ? (
                        <p
                          className={cn(
                            "mt-1 text-[11px] leading-relaxed",
                            !check.available || check.authenticated === false
                              ? "text-destructive"
                              : check.authenticated === true
                                ? "text-primary"
                                : "text-muted-foreground"
                          )}
                          role="status"
                          aria-live="polite"
                        >
                          {check.detail}
                        </p>
                      ) : null}
                      {agent.configured ? (
                        <Button
                          type="button"
                          size="xs"
                          variant="ghost"
                          className="mt-1.5"
                          disabled={sending || busy || externalBusy !== null || externalLoading}
                          onClick={() => void checkExternalAgent(agent)}
                        >
                          {busy && externalBusy?.action === "checking" ? "Checking..." : "Check CLI"}
                        </Button>
                      ) : null}
                      {agent.preset && agent.configured && agent.models.length ? (
                        <div className="mt-3 flex items-center justify-between gap-3 border-t border-border/50 pt-2.5">
                          <div className="min-w-0">
                            <div className="text-xs font-medium">Worker model</div>
                            <p className="mt-0.5 text-[11px] leading-relaxed text-muted-foreground">
                              Independent from Zest&apos;s chat model.
                            </p>
                          </div>
                          <WorkerModelPicker
                            workerLabel={agent.label}
                            model={agent.model}
                            models={agent.models}
                            disabled={
                              sending || busy || externalBusy !== null || externalLoading
                            }
                            onModelChange={(nextModel) =>
                              void setExternalAgentModel(agent, nextModel)
                            }
                          />
                        </div>
                      ) : null}
                      {agent.preset && agent.configured ? (
                        <div className="mt-3 flex items-center justify-between gap-3 border-t border-border/50 pt-2.5">
                          <div className="min-w-0">
                            <div className="text-xs font-medium">Optional MCP access</div>
                            <p className="mt-0.5 text-[11px] leading-relaxed text-muted-foreground">
                              Lets {agent.label} use the MCP servers already configured in its CLI.
                              This does not change provider routing.
                            </p>
                          </div>
                          <Button
                            type="button"
                            size="sm"
                            variant={agent.mcpAllowed ? "secondary" : "outline"}
                            disabled={sending || busy || externalBusy !== null || externalLoading}
                            aria-pressed={agent.mcpAllowed}
                            aria-label={`${agent.mcpAllowed ? "Turn off" : "Turn on"} MCP access for ${agent.label}`}
                            onClick={() => void toggleExternalAgentMcp(agent)}
                          >
                            {busy && externalBusy?.action === "mcp"
                              ? "Saving..."
                              : agent.mcpAllowed
                                ? "On"
                                : "Off"}
                          </Button>
                        </div>
                      ) : null}
                    </li>
                  );
                })}
                </ul>
              ) : (
                <p className="text-xs text-muted-foreground">
                  External workers could not be loaded. Try closing and reopening Settings.
                </p>
              )}
              {externalAgents[0] ? (
                <p className="mt-3 text-[11px] leading-relaxed text-muted-foreground">
                  Saved in {externalAgents[0].scope}. Delegations use an isolated worktree and
                  still require your approval before they run.
                </p>
              ) : null}
              {externalError ? (
                <p className="mt-2 text-xs text-destructive" role="alert">
                  {externalError}
                </p>
              ) : null}
            </div>
          </SettingsSection>

          <SettingsSection
            title="Usage"
            icon={ChartColumnIcon}
            hint={
              usage && (usage.providers.length || usage.externalWorkers.length)
                ? `${usage.providers.length + usage.externalWorkers.length} source${usage.providers.length + usage.externalWorkers.length === 1 ? "" : "s"}`
                : "Nothing used yet"
            }
          >
            {usage && (usage.providers.length || usage.externalWorkers.length) ? (
              <div className="space-y-2">
                {usage.providers.map((row) => (
                  <div
                    key={row.providerId}
                    className="rounded-lg border border-border/80 bg-card/80 px-3 py-2.5"
                  >
                    <div className="text-sm font-medium">{row.providerId}</div>
                    <div className="mt-1.5 space-y-1 text-[11px] text-muted-foreground">
                      <div>
                        <span className="text-foreground/80">Zest usage</span>
                        {": "}
                        {row.measured.requests}{" "}
                        {row.measured.requests === 1 ? "request" : "requests"} ·{" "}
                        {row.measured.totalTokens.toLocaleString()} tokens
                      </div>
                      <div>
                        {row.headroom.kind === "provider_reported" ? (
                          <>
                            <span className="text-foreground/80">Provider-reported limit</span>
                            {": "}
                            {row.headroom.requestsRemaining != null
                              ? `${row.headroom.requestsRemaining} left`
                              : "shared by your provider"}
                            {row.headroom.ageSecs != null
                              ? ` · updated ${formatAge(row.headroom.ageSecs)} ago`
                              : null}
                          </>
                        ) : (
                          <span className="text-foreground/80">
                            This provider did not report a limit
                          </span>
                        )}
                      </div>
                    </div>
                  </div>
                ))}
                {usage.externalWorkers.length ? (
                  <div className="pt-2">
                    <div className="mb-1.5 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
                      External workers
                    </div>
                    <div className="space-y-2">
                      {usage.externalWorkers.map((row) => (
                        <div
                          key={row.workerId}
                          className="rounded-lg border border-border/80 bg-card/80 px-3 py-2.5"
                        >
                          <div className="flex items-baseline justify-between gap-2">
                            <span className="text-sm font-medium">{row.workerId}</span>
                            <span className="text-[11px] tabular-nums text-muted-foreground">
                              {row.invocations} {row.invocations === 1 ? "run" : "runs"}
                            </span>
                          </div>
                          <div className="mt-1.5 space-y-1 text-[11px] text-muted-foreground">
                            <div>{externalTokenSummary(row)}</div>
                            <div>{externalContextSummary(row)}</div>
                            {row.lastCost ? (
                              <div>
                                Last reported cost: {row.lastCost.amount} {row.lastCost.currency}
                              </div>
                            ) : (
                              <div>Cost not reported</div>
                            )}
                          </div>
                        </div>
                      ))}
                    </div>
                    <p className="mt-2 text-[11px] leading-relaxed text-muted-foreground">
                      External worker figures come from the worker and are separate from Zest usage.
                    </p>
                  </div>
                ) : null}
                <p className="text-[11px] leading-relaxed text-muted-foreground">
                  Zest usage only — this does not show your provider plan balance.
                </p>
              </div>
            ) : (
              <p className="text-xs text-muted-foreground">
                {loading ? "Loading…" : "No usage yet. Send a message to start tracking."}
              </p>
            )}
          </SettingsSection>

          <SettingsSection
            title="Extras"
            icon={PuzzleIcon}
            hint={
              plugins.length
                ? `${plugins.filter((plugin) => plugin.enabled).length} on`
                : "None yet"
            }
          >
            <div className="flex flex-col gap-3">
              <div className="flex flex-wrap gap-1.5">
                <Button
                  type="button"
                  size="sm"
                  variant="secondary"
                  disabled={pluginFolderBusy}
                  onClick={() => void openPluginFolder()}
                >
                  <FolderOpenIcon data-icon="inline-start" aria-hidden="true" />
                  Open folder
                </Button>
                <Button type="button" size="sm" variant="outline" onClick={() => void refreshPlugins()}>
                  <RefreshCwIcon data-icon="inline-start" aria-hidden="true" />
                  Refresh
                </Button>
              </div>

              {plugins.length ? (
                <div className="flex flex-col gap-2">
                  {plugins.map((plugin) => (
                    <div
                      key={plugin.id}
                      className="rounded-lg border border-border/80 bg-card/80 px-3 py-2.5"
                    >
                      <div className="flex items-start justify-between gap-3">
                        <div className="min-w-0">
                          <div className="text-sm font-medium">{plugin.name}</div>
                          <p className="m-0 mt-1 text-[11px] leading-relaxed text-muted-foreground">
                            {plugin.description}
                          </p>
                        </div>
                        <Button
                          type="button"
                          size="sm"
                          variant={plugin.enabled ? "outline" : "default"}
                          disabled={!plugin.available || pluginBusy === plugin.id}
                          onClick={() => void togglePlugin(plugin)}
                        >
                          {pluginBusy === plugin.id
                            ? "Wait…"
                            : plugin.enabled
                              ? "Turn off"
                              : plugin.available
                                ? "Turn on"
                                : "Not ready"}
                        </Button>
                      </div>
                      {plugin.detail !== "Ready" ? (
                        <div className="mt-2 text-[10px] text-muted-foreground">
                          {plugin.detail}
                        </div>
                      ) : null}
                      {plugin.id === "now-playing" && plugin.enabled ? (
                        <NowPlayingCard value={nowPlaying} />
                      ) : null}
                    </div>
                  ))}
                </div>
              ) : (
                <p className="m-0 text-[11px] leading-relaxed text-muted-foreground">
                  Open the folder to add extras.
                </p>
              )}
              <p className="m-0 text-[11px] leading-relaxed text-muted-foreground">
                Extras stay on this PC.
              </p>
            </div>
          </SettingsSection>

          <SettingsSection title="System prompt" icon={ScrollTextIcon} hint={promptHint}>
            <p className="mb-2 text-xs leading-relaxed text-muted-foreground">
              Optional project instructions. Saved to{" "}
              <span className="font-mono text-[11px] text-foreground/80">{promptPath}</span>
              . Leave it blank to use Zest's default instructions.
              Takes effect on the next message.
            </p>
            {!customPrompt.trim() && basePrompt.trim() ? (
              <div className="mb-2 rounded-lg border border-border/60 bg-card/40 px-3 py-2">
                <div className="mb-1 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
                  Default (active while empty)
                </div>
                <p className="m-0 whitespace-pre-wrap font-mono text-[11px] leading-relaxed text-muted-foreground">
                  {basePrompt}
                </p>
              </div>
            ) : null}
            <textarea
              value={customPrompt}
              onChange={(e) => setCustomPrompt(e.target.value)}
              disabled={sending || promptSaving}
              rows={7}
              spellCheck={false}
              placeholder={"Optional: You are …\nProject conventions, tone, extra rules…"}
              className={cn(
                "w-full resize-y rounded-lg border border-border/80 bg-card/80 px-3 py-2 font-mono text-[11px] leading-relaxed text-foreground caret-foreground outline-none",
                "placeholder:text-muted-foreground/70 focus-visible:ring-2 focus-visible:ring-ring/50",
                "disabled:opacity-60"
              )}
            />
            <div className="mt-1.5 flex items-center justify-between gap-2 text-[11px] text-muted-foreground">
              <span className={overSoftLimit ? "text-destructive" : undefined}>
                {customPrompt.length.toLocaleString()} chars
                {overSoftLimit ? " · long prompts may use more context" : ""}
              </span>
              {promptSavedFlash ? (
                <span className="text-primary">Saved — next message uses it</span>
              ) : null}
            </div>
            {promptError ? (
              <p className="mt-1.5 text-xs text-destructive">{promptError}</p>
            ) : null}
            <div className="mt-2.5 flex flex-wrap gap-1.5">
              <Button
                type="button"
                size="sm"
                disabled={sending || promptSaving || !promptDirty}
                onClick={() => void savePrompt()}
              >
                {promptSaving ? "Saving…" : "Save"}
              </Button>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={sending || promptSaving || !promptDirty}
                onClick={revertPrompt}
              >
                Revert
              </Button>
            </div>
          </SettingsSection>

          <SettingsSection
            title="Skills"
            icon={BookOpenIcon}
            hint={
              skills.length === 0
                ? "None loaded"
                : `${skills.length} skill${skills.length === 1 ? "" : "s"}`
            }
          >
            <p className="mb-2 text-xs leading-relaxed text-muted-foreground">
              Your skills are kept on this computer. Zest loads <span className="font-mono text-[11px]">SKILL.md</span> files from{" "}
              <span className="font-mono text-[11px]">~/.agents/skills/</span> and{" "}
              <span className="font-mono text-[11px]">~/.zest/skills/</span>.
            </p>
            {skills.length === 0 ? (
              <p className="text-xs text-muted-foreground">No skills loaded.</p>
            ) : (
              <ul className="m-0 flex list-none flex-col gap-1.5 p-0">
                {skills.map((skill) => (
                  <li
                    key={skill.name}
                    className="rounded-md border border-border/70 bg-card/60 px-2.5 py-2"
                  >
                    <div className="flex items-baseline justify-between gap-2">
                      <span className="truncate text-sm font-medium">{skill.name}</span>
                      <span className="shrink-0 text-[10px] uppercase tracking-wide text-muted-foreground">
                        Personal
                      </span>
                    </div>
                    <p className="mt-0.5 text-[11px] leading-snug text-muted-foreground">
                      {skill.description}
                    </p>
                  </li>
                ))}
              </ul>
            )}
          </SettingsSection>

          <div ref={shortcutsRef}>
            <SettingsSection
              title="Keyboard shortcuts"
              icon={KeyboardIcon}
              hint="Rebind commands"
              openSignal={focusShortcuts}
            >
              <KeyboardShortcuts />
            </SettingsSection>
          </div>

          {error ? (
            <p className="px-4 py-3 text-xs text-destructive">{error}</p>
          ) : null}
          {loading ? (
            <p className="px-4 py-2 text-xs text-muted-foreground">Loading…</p>
          ) : null}
        </div>
      </div>
    </div>
  );
}
