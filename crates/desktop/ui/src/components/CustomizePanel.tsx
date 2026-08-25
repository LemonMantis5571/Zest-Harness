import {
  ArrowLeftIcon,
  BookOpenIcon,
  ChevronRightIcon,
  FolderOpenIcon,
  KeyboardIcon,
  PlugIcon,
  PlusIcon,
  PuzzleIcon,
  RefreshCwIcon,
  ScrollTextIcon,
  Trash2Icon,
  XIcon,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { KeyboardShortcuts } from "@/components/KeyboardShortcuts";
import { NowPlayingCard } from "@/components/NowPlayingCard";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { getBackend, type SkillSummary, type SystemPromptInfo } from "@/lib/backend";
import { ignoreExpectedFailure } from "@/lib/backgroundFailure";
import { messageFromError, validateMcpServerDraft } from "@/lib/mcpServerForm";
import type { CustomizeTab } from "@/lib/navigationHistory";
import type {
  ExternalAgentRow,
  McpServerRow,
  NowPlayingView,
  PluginView,
} from "@/lib/types";
import { cn } from "@/lib/utils";

type Props = {
  /** Whether MCP tools can reach this chat's provider at all. */
  providerOwnsAgentLoop: boolean;
  providerLabel: string;
  tab: CustomizeTab;
  sending: boolean;
  onTabChange: (tab: CustomizeTab) => void;
  onBack: () => void;
};

const TABS: { id: CustomizeTab; label: string; icon: typeof PlugIcon }[] = [
  { id: "mcp", label: "MCPs", icon: PlugIcon },
  { id: "skills", label: "Skills", icon: BookOpenIcon },
  { id: "plugins", label: "Extras", icon: PuzzleIcon },
  { id: "rules", label: "Rules", icon: ScrollTextIcon },
  { id: "shortcuts", label: "Shortcuts", icon: KeyboardIcon },
];

const CUSTOM_SOFT_LIMIT = 8000;
const DEFAULT_TIMEOUT_SECS = 120;

/** A server being added or edited. Split from `McpServerRow` because the form
 *  holds raw text — arguments as one line — that only becomes a row on save. */
type ServerDraft = {
  id: string;
  command: string;
  args: string;
  envVars: string;
  timeoutSecs: string;
  /** Set when editing, so the id field can stay locked. */
  editing: boolean;
};

function emptyDraft(): ServerDraft {
  return {
    id: "",
    command: "",
    args: "",
    envVars: "",
    timeoutSecs: String(DEFAULT_TIMEOUT_SECS),
    editing: false,
  };
}

function draftFrom(server: McpServerRow): ServerDraft {
  return {
    id: server.id,
    command: server.command,
    args: server.args.join(" "),
    envVars: server.envVars.join(", "),
    timeoutSecs: String(server.timeoutSecs),
    editing: true,
  };
}

export function CustomizePanel({
  providerOwnsAgentLoop,
  providerLabel,
  tab,
  sending,
  onTabChange,
  onBack,
}: Props) {
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      // Escape inside a field belongs to the field. Unlike the read-only
      // screens, this panel is mostly a form, and dismissing the whole page
      // while someone is typing a server command would lose their draft.
      const target = event.target as HTMLElement | null;
      const tag = target?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || target?.isContentEditable) {
        return;
      }
      onBack();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onBack]);

  useEffect(() => {
    rootRef.current?.focus();
  }, []);

  return (
    // Scrolls itself: this fills the transcript's place inside the chat shell,
    // which is a fixed-height column, so the page behind it never scrolls.
    <div
      ref={rootRef}
      tabIndex={-1}
      role="region"
      aria-label="Customize Zest"
      className="min-h-0 flex-1 overflow-y-auto outline-none animate-in fade-in duration-200"
    >
      <div className="mx-auto flex w-full max-w-[880px] flex-col gap-5 px-6 py-6">
        <header className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <h1 className="m-0 text-[19px] font-semibold leading-tight tracking-[-0.3px]">
              Customize
            </h1>
            <p className="m-0 mt-1 text-[12px] text-muted-foreground">
              Tools, skills, and instructions Zest uses in this project. Everything here stays
              on this computer.
            </p>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="shrink-0"
            onClick={onBack}
          >
            <ArrowLeftIcon className="size-3.5" />
            Back to chat
            <kbd
              aria-hidden
              className="ml-1.5 rounded border border-border/70 px-1 py-px font-mono text-[10px] leading-none text-muted-foreground"
            >
              Esc
            </kbd>
          </Button>
        </header>

        <nav
          aria-label="Customize sections"
          className="flex flex-wrap gap-1.5 border-b border-border/60 pb-3"
        >
          {TABS.map(({ id, label, icon: Icon }) => (
            <Button
              key={id}
              type="button"
              size="sm"
              variant={tab === id ? "secondary" : "ghost"}
              aria-current={tab === id ? "page" : undefined}
              onClick={() => onTabChange(id)}
            >
              <Icon data-icon="inline-start" aria-hidden="true" />
              {label}
            </Button>
          ))}
        </nav>

        {tab === "mcp" ? (
          <McpPanel
            providerOwnsAgentLoop={providerOwnsAgentLoop}
            providerLabel={providerLabel}
            sending={sending}
          />
        ) : null}
        {tab === "skills" ? <SkillsPanel /> : null}
        {tab === "plugins" ? <PluginsPanel /> : null}
        {tab === "rules" ? <RulesPanel sending={sending} /> : null}
        {tab === "shortcuts" ? <ShortcutsPanel /> : null}
      </div>
    </div>
  );
}

function SectionHeading({ title, hint }: { title: string; hint: string }) {
  return (
    <div>
      <h2 className="m-0 text-[13px] font-semibold tracking-[-0.1px]">{title}</h2>
      <p className="m-0 mt-1 text-[11px] leading-relaxed text-muted-foreground">{hint}</p>
    </div>
  );
}

function McpPanel({
  providerOwnsAgentLoop,
  providerLabel,
  sending,
}: {
  providerOwnsAgentLoop: boolean;
  providerLabel: string;
  sending: boolean;
}) {
  const [servers, setServers] = useState<McpServerRow[]>([]);
  const [agents, setAgents] = useState<ExternalAgentRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [checked, setChecked] = useState<Record<string, { ok: boolean; detail: string }>>({});
  const [draft, setDraft] = useState<ServerDraft | null>(null);
  const [draftError, setDraftError] = useState<string | null>(null);
  const [agentError, setAgentError] = useState<string | null>(null);

  const load = useCallback(async () => {
    const backend = getBackend();
    // Settled, not all: the CLI list and Zest's own servers are independent,
    // and one failing must not blank the other.
    const [serverResult, agentResult] = await Promise.allSettled([
      backend.listMcpServers(),
      backend.listExternalAgents(),
    ]);
    if (serverResult.status === "fulfilled") setServers(serverResult.value);
    else setError("Could not read your MCP servers. Try again.");
    if (agentResult.status === "fulfilled") setAgents(agentResult.value);
    setLoading(false);
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function save(next: ServerDraft, enabled: boolean) {
    const validated = validateMcpServerDraft(next);
    if (!validated.ok) {
      setDraftError(validated.error);
      return;
    }
    setDraftError(null);
    setBusy(validated.value.id);
    try {
      setServers(await getBackend().saveMcpServer({ ...validated.value, enabled }));
      setDraft(null);
      setError(null);
    } catch (err) {
      setDraftError(messageFromError(err, "Could not save this server."));
    } finally {
      setBusy(null);
    }
  }

  async function toggle(server: McpServerRow) {
    setBusy(server.id);
    try {
      setServers(await getBackend().setMcpServerEnabled(server.id, !server.enabled));
      setError(null);
    } catch (err) {
      setError(messageFromError(err, `Could not update ${server.id}.`));
    } finally {
      setBusy(null);
    }
  }

  async function remove(server: McpServerRow) {
    setBusy(server.id);
    try {
      setServers(await getBackend().removeMcpServer(server.id));
      setChecked((prev) => {
        const next = { ...prev };
        delete next[server.id];
        return next;
      });
      setError(null);
    } catch (err) {
      setError(messageFromError(err, `Could not remove ${server.id}.`));
    } finally {
      setBusy(null);
    }
  }

  async function check(server: McpServerRow) {
    setBusy(server.id);
    try {
      const result = await getBackend().checkMcpServer(server.id);
      setChecked((prev) => ({
        ...prev,
        [server.id]: { ok: result.ok, detail: result.detail },
      }));
      // A successful check is what puts the tools in front of the model, so the
      // rows have to be re-read rather than patched from the check alone.
      setServers(await getBackend().listMcpServers());
    } catch (err) {
      setChecked((prev) => ({
        ...prev,
        [server.id]: { ok: false, detail: messageFromError(err, "The check failed.") },
      }));
    } finally {
      setBusy(null);
    }
  }

  async function toggleAgentMcp(agent: ExternalAgentRow) {
    setBusy(agent.id);
    setAgentError(null);
    try {
      await getBackend().setExternalAgentMcp(agent.id, !agent.mcpAllowed);
      setAgents(await getBackend().listExternalAgents());
    } catch (err) {
      setAgentError(messageFromError(err, `Could not update MCP access for ${agent.label}.`));
    } finally {
      setBusy(null);
    }
  }

  const cliRows = useMemo(() => agents.filter((agent) => agent.preset), [agents]);

  return (
    <div className="flex flex-col gap-6">
      <section className="flex flex-col gap-3">
        <div className="flex items-start justify-between gap-3">
          <SectionHeading
            title="Zest's MCP servers"
            hint="Tools your chat provider can call directly. Every call still asks for your approval before it runs."
          />
          <Button
            type="button"
            size="sm"
            disabled={draft !== null}
            onClick={() => {
              setDraftError(null);
              setDraft(emptyDraft());
            }}
          >
            <PlusIcon data-icon="inline-start" aria-hidden="true" />
            Add server
          </Button>
        </div>

        {providerOwnsAgentLoop ? (
          <p
            role="status"
            className="m-0 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2.5 text-[11px] leading-relaxed text-amber-200/90"
          >
            {providerLabel} runs its own tools, so it loads MCP servers from its own CLI instead of
            these. Servers you add here apply to chats on an API provider.
          </p>
        ) : null}

        {draft ? (
          <ServerForm
            draft={draft}
            busy={busy !== null}
            error={draftError}
            onChange={setDraft}
            onCancel={() => {
              setDraft(null);
              setDraftError(null);
            }}
            onSave={(next) => void save(next, true)}
          />
        ) : null}

        {loading ? (
          <p className="m-0 text-[11px] text-muted-foreground" role="status">
            Loading servers…
          </p>
        ) : servers.length === 0 ? (
          <p className="m-0 rounded-lg border border-border/70 bg-card/50 px-3 py-4 text-[11px] leading-relaxed text-muted-foreground">
            No MCP servers yet. Add one to give this chat tools Zest does not ship with — a
            command like <span className="font-mono">npx -y @modelcontextprotocol/server-github</span>.
          </p>
        ) : (
          <ul className="m-0 flex list-none flex-col gap-2 p-0">
            {servers.map((server) => {
              const result = checked[server.id];
              const rowBusy = busy === server.id;
              return (
                <li
                  key={server.id}
                  className="rounded-lg border border-border/80 bg-card/70 px-3 py-2.5"
                >
                  <div className="flex items-start justify-between gap-2">
                    <div className="min-w-0">
                      <div className="truncate text-sm font-medium">{server.id}</div>
                      <div className="mt-0.5 text-[10px] uppercase tracking-wide text-muted-foreground">
                        {server.statusLabel}
                      </div>
                      <div className="mt-0.5 truncate font-mono text-[11px] text-muted-foreground">
                        {[server.command, ...server.args].join(" ")}
                      </div>
                    </div>
                    <Button
                      type="button"
                      size="sm"
                      variant={server.enabled ? "secondary" : "outline"}
                      disabled={sending || rowBusy}
                      aria-pressed={server.enabled}
                      aria-label={`${server.enabled ? "Turn off" : "Turn on"} the ${server.id} MCP server`}
                      onClick={() => void toggle(server)}
                    >
                      {server.enabled ? "On" : "Off"}
                    </Button>
                  </div>

                  <p className="m-0 mt-2 text-[11px] leading-relaxed text-muted-foreground">
                    {server.detail}
                  </p>
                  {server.tools.length ? (
                    <p className="m-0 mt-1 font-mono text-[10px] leading-relaxed text-muted-foreground/80">
                      {server.tools.join(" · ")}
                    </p>
                  ) : null}
                  {server.envVars.length ? (
                    <p className="m-0 mt-1 text-[10px] leading-relaxed text-muted-foreground/80">
                      Keeps {server.envVars.join(", ")} from your environment.
                    </p>
                  ) : null}
                  {result ? (
                    <p
                      role="status"
                      aria-live="polite"
                      className={cn(
                        "m-0 mt-1 text-[11px] leading-relaxed",
                        result.ok ? "text-primary" : "text-destructive"
                      )}
                    >
                      {result.detail}
                    </p>
                  ) : null}

                  <div className="mt-2 flex flex-wrap gap-1.5">
                    <Button
                      type="button"
                      size="xs"
                      variant="ghost"
                      disabled={sending || rowBusy || !server.enabled}
                      onClick={() => void check(server)}
                    >
                      <RefreshCwIcon data-icon="inline-start" aria-hidden="true" />
                      {rowBusy ? "Checking…" : "Check server"}
                    </Button>
                    <Button
                      type="button"
                      size="xs"
                      variant="ghost"
                      disabled={sending || rowBusy || draft !== null}
                      onClick={() => {
                        setDraftError(null);
                        setDraft(draftFrom(server));
                      }}
                    >
                      Edit
                    </Button>
                    <Button
                      type="button"
                      size="xs"
                      variant="ghost"
                      disabled={sending || rowBusy}
                      onClick={() => void remove(server)}
                    >
                      <Trash2Icon data-icon="inline-start" aria-hidden="true" />
                      Remove
                    </Button>
                  </div>
                </li>
              );
            })}
          </ul>
        )}

        {servers[0] ? (
          <p className="m-0 text-[11px] leading-relaxed text-muted-foreground">
            Saved in {servers[0].scope}. A server starts only when a chat calls one of its tools.
          </p>
        ) : null}
        {error ? (
          <p className="m-0 text-[11px] text-destructive" role="alert">
            {error}
          </p>
        ) : null}
      </section>

      <section className="flex flex-col gap-3 border-t border-border/60 pt-5">
        <SectionHeading
          title="CLI worker MCP access"
          hint="Workers you delegate to can also use the MCP servers configured in their own CLI. Zest does not see or approve those calls, so this stays a separate switch."
        />
        {cliRows.length === 0 ? (
          <p className="m-0 text-[11px] text-muted-foreground">
            No CLI workers are available here.
          </p>
        ) : (
          <ul className="m-0 flex list-none flex-col gap-2 p-0">
            {cliRows.map((agent) => (
              <li
                key={agent.id}
                className="flex items-start justify-between gap-3 rounded-lg border border-border/80 bg-card/70 px-3 py-2.5"
              >
                <div className="min-w-0">
                  <div className="truncate text-sm font-medium">{agent.label}</div>
                  <p className="m-0 mt-0.5 text-[11px] leading-relaxed text-muted-foreground">
                    {agent.configured
                      ? `Uses the MCP servers already set up in the ${agent.label} CLI.`
                      : "Enable delegation for this worker in Settings first."}
                  </p>
                </div>
                <Button
                  type="button"
                  size="sm"
                  variant={agent.mcpAllowed ? "secondary" : "outline"}
                  disabled={sending || busy === agent.id || !agent.configured}
                  aria-pressed={agent.mcpAllowed}
                  aria-label={`${agent.mcpAllowed ? "Turn off" : "Turn on"} MCP access for ${agent.label}`}
                  onClick={() => void toggleAgentMcp(agent)}
                >
                  {busy === agent.id ? "Saving…" : agent.mcpAllowed ? "On" : "Off"}
                </Button>
              </li>
            ))}
          </ul>
        )}
        {agentError ? (
          <p className="m-0 text-[11px] text-destructive" role="alert">
            {agentError}
          </p>
        ) : null}
      </section>
    </div>
  );
}

function ServerForm({
  draft,
  busy,
  error,
  onChange,
  onCancel,
  onSave,
}: {
  draft: ServerDraft;
  busy: boolean;
  error: string | null;
  onChange: (draft: ServerDraft) => void;
  onCancel: () => void;
  onSave: (draft: ServerDraft) => void;
}) {
  return (
    <form
      className="flex flex-col gap-2.5 rounded-lg border border-border/80 bg-card/80 px-3 py-3"
      onSubmit={(event) => {
        event.preventDefault();
        onSave(draft);
      }}
    >
      <div className="flex items-center justify-between gap-2">
        <span className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
          {draft.editing ? `Edit ${draft.id}` : "New MCP server"}
        </span>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          title="Cancel"
          aria-label="Cancel"
          onClick={onCancel}
        >
          <XIcon />
        </Button>
      </div>

      <Field label="Name" hint="Used in the tool names the model sees.">
        <Input
          value={draft.id}
          disabled={draft.editing || busy}
          spellCheck={false}
          placeholder="github"
          onChange={(event) => onChange({ ...draft, id: event.target.value })}
        />
      </Field>
      <Field label="Command" hint="Run directly, without a shell.">
        <Input
          value={draft.command}
          disabled={busy}
          spellCheck={false}
          placeholder="npx"
          onChange={(event) => onChange({ ...draft, command: event.target.value })}
        />
      </Field>
      <Field label="Arguments" hint="Separated by spaces. Quote anything containing one.">
        <Input
          value={draft.args}
          disabled={busy}
          spellCheck={false}
          placeholder="-y @modelcontextprotocol/server-github"
          onChange={(event) => onChange({ ...draft, args: event.target.value })}
        />
      </Field>
      <Field
        label="Environment variables"
        hint="Names only. The values stay in your environment — never write a token here."
      >
        <Input
          value={draft.envVars}
          disabled={busy}
          spellCheck={false}
          placeholder="GITHUB_TOKEN"
          onChange={(event) => onChange({ ...draft, envVars: event.target.value })}
        />
      </Field>
      <Field label="Timeout (seconds)" hint="How long one tool call may take.">
        <Input
          value={draft.timeoutSecs}
          disabled={busy}
          inputMode="numeric"
          onChange={(event) => onChange({ ...draft, timeoutSecs: event.target.value })}
        />
      </Field>

      {error ? (
        <p className="m-0 text-[11px] text-destructive" role="alert">
          {error}
        </p>
      ) : null}
      <div className="flex flex-wrap gap-1.5">
        <Button type="submit" size="sm" disabled={busy}>
          {busy ? "Saving…" : "Save and check"}
        </Button>
        <Button type="button" size="sm" variant="outline" disabled={busy} onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-[11px] font-medium">{label}</span>
      {children}
      <span className="text-[10px] leading-relaxed text-muted-foreground">{hint}</span>
    </label>
  );
}

function PluginsPanel() {
  const [plugins, setPlugins] = useState<PluginView[]>([]);
  const [nowPlaying, setNowPlaying] = useState<NowPlayingView | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [folderBusy, setFolderBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    const backend = getBackend();
    try {
      const [rows, music] = await Promise.all([backend.listPlugins(), backend.nowPlaying()]);
      setPlugins(rows);
      setNowPlaying(music);
      setError(null);
    } catch {
      setError("Could not read your extras. Try again.");
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // Media metadata is polled only while this tab is open and the user has
  // opted into the plugin. No background listener outlives the panel.
  useEffect(() => {
    if (!plugins.some((plugin) => plugin.id === "now-playing" && plugin.enabled)) return;
    const timer = window.setInterval(() => {
      void getBackend()
        .nowPlaying()
        .then(setNowPlaying)
        .catch((error) => ignoreExpectedFailure(error, "refresh now playing metadata"));
    }, 5_000);
    return () => window.clearInterval(timer);
  }, [plugins]);

  async function toggle(plugin: PluginView) {
    setBusy(plugin.id);
    try {
      setPlugins(await getBackend().setPluginEnabled(plugin.id, !plugin.enabled));
      setNowPlaying(await getBackend().nowPlaying());
      setError(null);
    } catch (err) {
      setError(messageFromError(err, "Could not change this extra."));
    } finally {
      setBusy(null);
    }
  }

  async function openFolder() {
    setFolderBusy(true);
    try {
      await getBackend().openPluginsFolder();
    } catch {
      setError("Could not open the extras folder.");
    } finally {
      setFolderBusy(false);
    }
  }

  return (
    <section className="flex flex-col gap-3">
      <div className="flex items-start justify-between gap-3">
        <SectionHeading
          title="Extras"
          hint="Optional local integrations. They stay on this PC, and each one is off until you turn it on."
        />
        <div className="flex shrink-0 flex-wrap gap-1.5">
          <Button
            type="button"
            size="sm"
            variant="secondary"
            disabled={folderBusy}
            onClick={() => void openFolder()}
          >
            <FolderOpenIcon data-icon="inline-start" aria-hidden="true" />
            Open folder
          </Button>
          <Button type="button" size="sm" variant="outline" onClick={() => void load()}>
            <RefreshCwIcon data-icon="inline-start" aria-hidden="true" />
            Refresh
          </Button>
        </div>
      </div>

      {plugins.length ? (
        <div className="flex flex-col gap-2">
          {plugins.map((plugin) => (
            <div
              key={plugin.id}
              className="rounded-lg border border-border/80 bg-card/70 px-3 py-2.5"
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
                  disabled={!plugin.available || busy === plugin.id}
                  onClick={() => void toggle(plugin)}
                >
                  {busy === plugin.id
                    ? "Wait…"
                    : plugin.enabled
                      ? "Turn off"
                      : plugin.available
                        ? "Turn on"
                        : "Not ready"}
                </Button>
              </div>
              {plugin.detail !== "Ready" ? (
                <div className="mt-2 text-[10px] text-muted-foreground">{plugin.detail}</div>
              ) : null}
              {plugin.id === "now-playing" && plugin.enabled ? (
                <NowPlayingCard value={nowPlaying} />
              ) : null}
            </div>
          ))}
        </div>
      ) : (
        <p className="m-0 rounded-lg border border-border/70 bg-card/50 px-3 py-4 text-[11px] leading-relaxed text-muted-foreground">
          No extras installed. Open the folder to add one.
        </p>
      )}
      {error ? (
        <p className="m-0 text-[11px] text-destructive" role="alert">
          {error}
        </p>
      ) : null}
    </section>
  );
}

function ShortcutsPanel() {
  return (
    <section className="flex flex-col gap-3">
      <SectionHeading
        title="Keyboard shortcuts"
        hint="Rebind any command. Mod is Ctrl on Windows and Linux, Cmd on macOS."
      />
      <KeyboardShortcuts />
    </section>
  );
}

function SkillsPanel() {
  const [skills, setSkills] = useState<SkillSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setSkills(await getBackend().listSkills());
      setError(null);
    } catch {
      setError("Could not load skills. Try again.");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <section className="flex flex-col gap-3">
      <div className="flex items-start justify-between gap-3">
        <SectionHeading
          title="Skills"
          hint="Zest loads SKILL.md files from ~/.agents/skills/ and ~/.zest/skills/. Each one is also available as a slash command."
        />
        <Button type="button" size="sm" variant="outline" onClick={() => void load()}>
          <RefreshCwIcon data-icon="inline-start" aria-hidden="true" />
          Refresh
        </Button>
      </div>
      {loading ? (
        <p className="m-0 text-[11px] text-muted-foreground" role="status">
          Loading skills…
        </p>
      ) : skills.length === 0 ? (
        <p className="m-0 rounded-lg border border-border/70 bg-card/50 px-3 py-4 text-[11px] leading-relaxed text-muted-foreground">
          No skills loaded. Add a folder with a <span className="font-mono">SKILL.md</span> in it
          under <span className="font-mono">~/.zest/skills/</span>.
        </p>
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
              <p className="m-0 mt-0.5 text-[11px] leading-snug text-muted-foreground">
                {skill.description}
              </p>
            </li>
          ))}
        </ul>
      )}
      {error ? (
        <p className="m-0 text-[11px] text-destructive" role="alert">
          {error}
        </p>
      ) : null}
    </section>
  );
}

function RulesPanel({ sending }: { sending: boolean }) {
  const [custom, setCustom] = useState("");
  const [saved, setSaved] = useState("");
  const [basePrompt, setBasePrompt] = useState("");
  const [path, setPath] = useState(".zest/system.md");
  const [saving, setSaving] = useState(false);
  const [savedFlash, setSavedFlash] = useState(false);
  const [showBase, setShowBase] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    void getBackend()
      .getSystemPrompt()
      .then((info: SystemPromptInfo) => {
        if (!live) return;
        setCustom(info.custom);
        setSaved(info.custom);
        setBasePrompt(info.base);
        setPath(info.customPath);
      })
      .catch(() => {
        if (live) setError("Could not read your project instructions. Try again.");
      });
    return () => {
      live = false;
    };
  }, []);

  const dirty = custom !== saved;
  const overSoftLimit = custom.length > CUSTOM_SOFT_LIMIT;

  async function save() {
    setSaving(true);
    setError(null);
    try {
      const info = await getBackend().setSystemPrompt(custom);
      setCustom(info.custom);
      setSaved(info.custom);
      setBasePrompt(info.base);
      setPath(info.customPath);
      setSavedFlash(true);
      window.setTimeout(() => setSavedFlash(false), 2500);
    } catch (err) {
      setError(messageFromError(err, "Could not save your project instructions."));
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="flex flex-col gap-3">
      <SectionHeading
        title="Project instructions"
        hint={`Saved to ${path}. Leave it blank to use Zest's defaults. Takes effect on your next message.`}
      />
      {!custom.trim() && basePrompt.trim() ? (
        // Collapsed, and scrolling when open. Zest's default instructions run
        // to dozens of lines, and rendering them in full pushed the box you
        // came here to type in off the bottom of the page.
        <div className="rounded-lg border border-border/60 bg-card/40">
          <button
            type="button"
            aria-expanded={showBase}
            onClick={() => setShowBase((value) => !value)}
            className="flex w-full cursor-pointer items-center gap-2 px-3 py-2 text-left outline-none transition-colors hover:bg-accent/30 focus-visible:ring-2 focus-visible:ring-ring/40"
          >
            <ChevronRightIcon
              className={cn(
                "size-3 shrink-0 text-muted-foreground transition-transform duration-150",
                showBase && "rotate-90"
              )}
              aria-hidden
            />
            <span className="text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
              Default (active while empty)
            </span>
            <span className="ml-auto text-[10px] text-muted-foreground/70">
              {basePrompt.length.toLocaleString()} chars
            </span>
          </button>
          {showBase ? (
            <p className="m-0 max-h-64 overflow-y-auto whitespace-pre-wrap border-t border-border/50 px-3 py-2 font-mono text-[11px] leading-relaxed text-muted-foreground">
              {basePrompt}
            </p>
          ) : null}
        </div>
      ) : null}
      <textarea
        value={custom}
        onChange={(event) => setCustom(event.target.value)}
        disabled={sending || saving}
        rows={14}
        spellCheck={false}
        placeholder={"Optional: You are …\nProject conventions, tone, extra rules…"}
        className={cn(
          "w-full resize-y rounded-lg border border-border/80 bg-card/80 px-3 py-2 font-mono text-[11px] leading-relaxed text-foreground caret-foreground outline-none",
          "placeholder:text-muted-foreground/70 focus-visible:ring-2 focus-visible:ring-ring/50",
          "disabled:opacity-60"
        )}
      />
      <div className="flex items-center justify-between gap-2 text-[11px] text-muted-foreground">
        <span className={overSoftLimit ? "text-destructive" : undefined}>
          {custom.length.toLocaleString()} chars
          {overSoftLimit ? " · long instructions may use more context" : ""}
        </span>
        {savedFlash ? (
          <span className="text-primary">Saved — your next message uses it</span>
        ) : null}
      </div>
      {error ? (
        <p className="m-0 text-[11px] text-destructive" role="alert">
          {error}
        </p>
      ) : null}
      <div className="flex flex-wrap gap-1.5">
        <Button
          type="button"
          size="sm"
          disabled={sending || saving || !dirty}
          onClick={() => void save()}
        >
          {saving ? "Saving…" : "Save"}
        </Button>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={sending || saving || !dirty}
          onClick={() => setCustom(saved)}
        >
          Revert
        </Button>
      </div>
    </section>
  );
}
