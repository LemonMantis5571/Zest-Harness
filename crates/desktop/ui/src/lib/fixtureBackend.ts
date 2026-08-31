/**
 * The offline fixture backend.
 *
 * Its own module so production builds can drop it. It used to live beside the
 * desktop backend and be chosen at runtime from `?fixture`, which meant ~690
 * lines of canned data shipped inside the entry chunk of every release to
 * serve a query parameter that only makes sense on a dev server.
 *
 * `selectBackend` now reaches for this only under `import.meta.env.DEV`, a
 * compile-time constant, so the import is statically dead in a production
 * build and the bundler removes the whole module.
 */
import { runFixtureStream } from "./fixture.ts";
import { safeMarkdownFilename } from "./markdownExport.ts";
import { CODEX_MODELS, DEFAULT_CODEX_MODEL, DEFAULT_EFFORT } from "./models.ts";
import { matchExcerpt } from "./commandPaletteSearch.ts";
import {
  THREAD_OLDER_USER_TURNS,
  THREAD_WINDOW_USER_TURNS,
  aroundUserTurns,
  newerUserTurns,
  olderUserTurns,
  tailUserTurns,
} from "./threadWindow.ts";
import type { DesktopBackend } from "./backend";
import type {
  ApprovalMode,
  AttachmentInput,
  ChatEvent,
  ChatMessage,
  DelegationEvent,
  DelegationCreateInput,
  DelegationJob,
  DelegationTargetOptionView,
  DelegationUpdateInput,
  GitContext,
  InputTarget,
  JobRead,
  JobSnapshot,
  McpServerRow,
  OlderThreadMessages,
  SessionInfo,
  ThreadSummary,
  ChatSearchHit,
  WallpaperFilterId,
  WorkspaceChange,
} from "./types";

const FIXTURE_MODELS = CODEX_MODELS.map((m) => ({
  id: m.id,
  efforts: ["low", "medium", "high", "xhigh", "max"],
  contextWindow: 256000,
  supportsTools: true,
  supportsVision: false,
}));
const FIXTURE_ARTWORK_DATA_URL =
  "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 96 96'%3E%3Cdefs%3E%3ClinearGradient id='g' x1='0' y1='0' x2='1' y2='1'%3E%3Cstop stop-color='%235e6ad2'/%3E%3Cstop offset='1' stop-color='%23c084fc'/%3E%3C/linearGradient%3E%3C/defs%3E%3Crect width='96' height='96' rx='16' fill='url(%23g)'/%3E%3Ccircle cx='48' cy='48' r='22' fill='%23010102' opacity='.8'/%3E%3Ccircle cx='48' cy='48' r='7' fill='%23f4f4f5'/%3E%3C/svg%3E";
/** One stand-in per look, so the fixture UI shows a real difference. */
const FIXTURE_WALLPAPER_PREVIEWS: Record<WallpaperFilterId, string> = {
  none: "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 64 64'%3E%3Cdefs%3E%3ClinearGradient id='w' x1='0' y1='0' x2='1' y2='1'%3E%3Cstop stop-color='%231c1d20'/%3E%3Cstop offset='1' stop-color='%235e6ad2'/%3E%3C/linearGradient%3E%3C/defs%3E%3Crect width='64' height='64' fill='url(%23w)'/%3E%3C/svg%3E",
  print:
    "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 8 8'%3E%3Crect width='8' height='8' fill='%231c1d20'/%3E%3Cpath fill='%235e6ad2' d='M0 0h1v1H0zm2 0h1v1H2zm4 0h1v1H4zm6 0h1v1H6zM1 1h1v1H1zm3 0h1v1H3zm5 0h1v1H5zm7 0h1v1H7zM0 2h1v1H0zm2 0h1v1H2zm4 0h1v1H4zm6 0h1v1H6zM1 3h1v1H1zm3 0h1v1H3zm5 0h1v1H5zm7 0h1v1H7z'/%3E%3Cpath fill='%23c4b5e0' d='M0 4h1v1H0zm2 0h1v1H2zm4 0h1v1H4zm6 0h1v1H6zM1 5h1v1H1zm3 0h1v1H3zm5 0h1v1H5zm7 0h1v1H7zM0 6h1v1H0zm2 0h1v1H2zm4 0h1v1H4zm6 0h1v1H6zM1 7h1v1H1zm3 0h1v1H3zm5 0h1v1H5zm7 0h1v1H7z'/%3E%3C/svg%3E",
  frosted:
    "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 64 64'%3E%3Cdefs%3E%3Cfilter id='f'%3E%3CfeGaussianBlur stdDeviation='7'/%3E%3C/filter%3E%3C/defs%3E%3Crect width='64' height='64' fill='%231c1d20'/%3E%3Cg filter='url(%23f)'%3E%3Ccircle cx='20' cy='22' r='16' fill='%235e6ad2'/%3E%3Ccircle cx='46' cy='44' r='18' fill='%23c084fc'/%3E%3C/g%3E%3C/svg%3E",
  noir: "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 64 64'%3E%3Cdefs%3E%3ClinearGradient id='n' x1='0' y1='0' x2='1' y2='1'%3E%3Cstop stop-color='%23161616'/%3E%3Cstop offset='1' stop-color='%23d4d4d4'/%3E%3C/linearGradient%3E%3C/defs%3E%3Crect width='64' height='64' fill='url(%23n)'/%3E%3C/svg%3E",
};
const FIXTURE_SESSION: SessionInfo = {
  sessionId: "session-fixture",
  provider: "fixture",
  label: "Fixture",
  model: DEFAULT_CODEX_MODEL,
  effort: DEFAULT_EFFORT,
  root: ".",
  isFreeChat: false,
  threadId: "fixture",
  defaultModel: DEFAULT_CODEX_MODEL,
  // The fixture stands in for an API provider, so Zest owns the loop and the
  // Customize screen shows MCP servers as applying here.
  ownsAgentLoop: false,
  models: FIXTURE_MODELS,
  checkpoints: [],
  pendingInputs: [],
  messages: [],
  hasOlderMessages: false,
  hasNewerMessages: false,
  hiddenUserTurns: 0,
};

function notAvailable(op: string): never {
  throw new Error(`fixture backend: ${op} is not available`);
}

const LONG_THREAD_ID = "fixture-long";

function longThreadMessages(): ChatMessage[] {
  const messages: ChatMessage[] = [];
  for (let index = 1; index <= 15; index += 1) {
    messages.push({
      id: `long-u${index}`,
      role: "user",
      text: `Turn ${index} prompt`,
    });
    messages.push({
      id: `long-a${index}`,
      role: "assistant",
      text: `Turn ${index} reply`,
      thinking: "",
      tools: [],
      streaming: false,
    });
  }
  return messages;
}

const MAX_FIXTURE_THREAD_TITLE_CHARS = 200;

export type FixtureScenario = "approval" | "question" | "cancel" | "tool-error";

type FixtureBackendOptions = {
  scenario?: FixtureScenario;
};

function scenarioFromLocation(): FixtureScenario | undefined {
  if (typeof window === "undefined") return undefined;
  const value = new URLSearchParams(window.location.search).get("scenario");
  return value === "approval" ||
    value === "question" ||
    value === "cancel" ||
    value === "tool-error"
    ? value
    : undefined;
}

type FixturePendingScenario = {
  kind: "approval" | "question" | "cancel";
  turnId: string;
  assistantId: string;
  toolId: string;
  approvalId?: string;
  questionId?: string;
};

export function createFixtureBackend(options: FixtureBackendOptions = {}): DesktopBackend {
  const scenario = options.scenario ?? scenarioFromLocation();
  let session: SessionInfo = { ...FIXTURE_SESSION, messages: [] };
  let chatHandler: ((event: ChatEvent) => void) | null = null;
  let chatHandlerGeneration = 0;
  let workspace = ".";
  let fixturePinned = false;
  const fixtureThreadTitles = new Map<string, string>([
    ["fixture", "Fixture"],
    ["fixture-local", "Local model chat"],
    ["fixture-free", "Free chat"],
    [LONG_THREAD_ID, "Fifteen turns"],
  ]);
  const fixtureThreadBodies = new Map<string, string>([
    [
      "fixture-local",
      "Please git pull the latest OceanicUI component branch and open a pull request.",
    ],
    ["fixture-free", "Hi — can you sketch a webring homepage?"],
  ]);
  const fixtureTranscripts = new Map<string, ChatMessage[]>([
    [LONG_THREAD_ID, longThreadMessages()],
  ]);

  function windowSessionFor(
    threadId: string,
    extra: Partial<SessionInfo> = {},
    focusMessageId?: string | null
  ): SessionInfo {
    const full = fixtureTranscripts.get(threadId) ?? [];
    const window = focusMessageId
      ? aroundUserTurns(full, focusMessageId, THREAD_WINDOW_USER_TURNS)
      : tailUserTurns(full, THREAD_WINDOW_USER_TURNS);
    return {
      ...session,
      ...extra,
      threadId,
      messages: window.messages,
      hasOlderMessages: window.hasOlder,
      hasNewerMessages: window.hasNewer,
      hiddenUserTurns: window.hiddenUserTurns,
      focusMessageId: focusMessageId || undefined,
    };
  }
  const enabledExternalAgents = new Set<string>();
  const fixtureMcpAgents = new Set<string>();
  const fixtureExternalModels = new Map<string, string>();
  /** Zest-owned MCP servers. One row so the Customize screen has something to
   *  render offline, including the never-checked state. */
  const fixtureMcpServers = new Map<string, McpServerRow>([
    [
      "Haiku",
      {
        id: "Haiku",
        command: "npx",
        args: ["-y", "haiku-mcp"],
        url: "",
        headers: {},
        envVars: [],
        enabled: true,
        timeoutSecs: 120,
        scope: "the fixture config",
        tools: ["manifest"],
        statusLabel: "Ready",
        detail: "1 tool · checked just now",
      },
    ],
    [
      "github",
      {
        id: "github",
        command: "npx",
        args: ["-y", "@modelcontextprotocol/server-github"],
        url: "",
        headers: {},
        envVars: ["GITHUB_TOKEN"],
        enabled: true,
        timeoutSecs: 120,
        scope: "the fixture config",
        tools: ["search_issues", "create_issue"],
        statusLabel: "Ready",
        detail: "2 tools · checked just now",
      },
    ],
  ]);
  let fixtureNowPlayingEnabled = false;
  let fixtureNowPlayingStatus: "playing" | "paused" = "playing";
  let fixtureNowPlayingVolume = 83;
  let fixtureWallpaperEnabled = false;
  let fixtureWallpaperChosen = false;
  let fixtureWallpaperFilter: WallpaperFilterId = "none";
  let pendingScenario: FixturePendingScenario | null = null;
  let cancelTimer: ReturnType<typeof setTimeout> | null = null;

  function clearFixtureScenario() {
    pendingScenario = null;
    if (cancelTimer !== null) {
      clearTimeout(cancelTimer);
      cancelTimer = null;
    }
  }

  const fixtureExternalModelOptions: Record<string, string[]> = {
    claude: ["sonnet", "opus"],
    gemini: [
      "auto",
      "gemini-3-pro-preview",
      "gemini-3-flash-preview",
      "gemini-2.5-pro",
      "gemini-2.5-flash",
    ],
  };

  let delegationHandler: ((event: DelegationEvent) => void) | null = null;
  let delegationHandlerGeneration = 0;
  let fixtureDelegationJob: DelegationJob = {
    jobId: "fixture-delegation-1",
    parentThreadId: session.threadId,
    projectRoot: workspace,
    cardId: "fixture-card-1",
    title: "Add the fixture lifecycle",
    objective: "Exercise the coordinator, worker, reviewer, and apply handoff.",
    lane: "product",
    scope: ["src"],
    context: ["README.md"],
    dependsOn: [],
    agent: "claude",
    reviewerAgent: "claude",
    workerTarget: { kind: "externalAgent", agentId: "claude" },
    reviewerTarget: { kind: "sameAsWorker" },
    resolvedWorkerTarget: null,
    resolvedReviewerTarget: null,
    approved: false,
    origin: { coordinator: "fixture", chatId: null, threadId: session.threadId },
    attempts: [],
    attempt: 0,
    status: "awaiting_approval",
    changedFiles: [],
    changedFileCount: 0,
    acceptanceChecks: [
      { command: "npm test", status: "pending", output: "" },
    ],
    reviewerFindings: [],
    workerSummary: undefined,
    error: undefined,
    createdAt: 1_700_000_000_000,
    updatedAt: 1_700_000_000_000,
  };

  function delegationSnapshot(): DelegationJob {
    return {
      ...fixtureDelegationJob,
      scope: [...fixtureDelegationJob.scope],
      context: [...fixtureDelegationJob.context],
      dependsOn: [...fixtureDelegationJob.dependsOn],
      changedFiles: [...fixtureDelegationJob.changedFiles],
      acceptanceChecks: fixtureDelegationJob.acceptanceChecks.map((check) => ({ ...check })),
      reviewerFindings: fixtureDelegationJob.reviewerFindings.map((finding) => ({ ...finding })),
    };
  }

  function emitDelegation(kind: DelegationEvent["kind"]) {
    delegationHandler?.({ kind, job: delegationSnapshot() } as DelegationEvent);
  }

  function updateDelegation(
    status: DelegationJob["status"],
    kind: DelegationEvent["kind"],
    update: Partial<DelegationJob> = {}
  ) {
    fixtureDelegationJob = {
      ...fixtureDelegationJob,
      ...update,
      status,
      updatedAt: fixtureDelegationJob.updatedAt + 1,
    };
    emitDelegation(kind);
  }

  async function runFixtureDelegation(jobId: string) {
    if (jobId !== fixtureDelegationJob.jobId) {
      throw new Error(`fixture backend: delegation job ${jobId} was not found`);
    }
    if (
      !["awaiting_approval", "changes_requested", "blocked", "failed"].includes(
        fixtureDelegationJob.status
      )
    ) {
      return delegationSnapshot();
    }
    updateDelegation("worker_running", "worker_started", {
      attempt: fixtureDelegationJob.attempt + 1,
      workerAttemptId: `fixture-worker-${fixtureDelegationJob.attempt + 1}`,
      error: undefined,
    });
    updateDelegation("worker_running", "worker_completed", {
      changedFiles: ["src/fixture.ts"],
      changedFileCount: 1,
      workerSummary: "Worker created a deterministic fixture change.",
      acceptanceChecks: [
        { command: "npm test", status: "passed", output: "fixture tests passed" },
      ],
    });
    updateDelegation("review_running", "reviewer_started", {
      reviewerAttemptId: `fixture-reviewer-${fixtureDelegationJob.attempt}`,
    });
    updateDelegation("review_running", "reviewer_completed");
    updateDelegation("ready_to_apply", "ready_to_apply");
    return delegationSnapshot();
  }

  function fixtureIds() {
    return {
      turnId: `turn-${crypto.randomUUID()}`,
      userId: `user-${crypto.randomUUID()}`,
      assistantId: `assistant-${crypto.randomUUID()}`,
    };
  }

  function displayFixtureText(text: string, attachments?: AttachmentInput[]) {
    let display = text.trim();
    if (attachments?.length) {
      const lines = attachments.map((a) => `Attached: ${a.name} (${a.detail})`);
      display = display ? `${display}\n\n${lines.join("\n")}` : lines.join("\n");
    }
    return display;
  }

  function emitFixtureEcho(text: string, attachments?: AttachmentInput[]) {
    if (!chatHandler) return;
    const { turnId, userId, assistantId } = fixtureIds();
    const id = {
      session_id: session.sessionId,
      thread_id: session.threadId,
      turn_id: turnId,
    };
    const display = displayFixtureText(text, attachments);
    const fixtureAssistantText = `Fixture echo: ${text.trim() || "(attachment)"}`;
    session = {
      ...session,
      messages: [
        ...session.messages,
        {
          id: userId,
          role: "user",
          text: display,
          attachments: attachments?.length
            ? attachments.map((attachment) => ({
                name: attachment.name,
                kind: attachment.kind ?? "file",
              }))
            : undefined,
        },
        {
          id: assistantId,
          role: "assistant",
          text: fixtureAssistantText,
          thinking: "",
          tools: [],
          streaming: false,
        },
      ],
    };
    chatHandler({ kind: "user", ...id, message_id: userId, text: display });
    chatHandler({ kind: "assistant_start", ...id, message_id: assistantId });
    chatHandler({
      kind: "text_delta",
      ...id,
      message_id: assistantId,
      text: "Fixture echo: ",
    });
    chatHandler({
      kind: "text_delta",
      ...id,
      message_id: assistantId,
      text: text.trim() || "(attachment)",
    });
    chatHandler({ kind: "done", ...id, message_id: assistantId });
  }

  function finishScenario(kind: "allow" | "deny" | "answer" | "cancel") {
    const pending = pendingScenario;
    if (!pending || !chatHandler) return false;
    clearFixtureScenario();
    const id = {
      session_id: session.sessionId,
      thread_id: session.threadId,
      turn_id: pending.turnId,
      message_id: pending.assistantId,
    };
    if (kind === "cancel") {
      chatHandler({ kind: "cancelled", ...id });
      return true;
    }
    const denied = kind === "deny";
    const summary =
      kind === "answer"
        ? "Fixture question answered."
        : denied
          ? "Fixture approval denied."
          : "Fixture approval allowed.";
    chatHandler({
      kind: "tool_call_result",
      ...id,
      name: "fixture_tool",
      id: pending.toolId,
      summary,
      isError: denied,
    });
    chatHandler({
      kind: "text_delta",
      ...id,
      text: denied
        ? "Fixture stopped after the approval was denied."
        : kind === "answer"
          ? "Fixture received the answer."
          : "Fixture continued after approval.",
    });
    chatHandler({ kind: "done", ...id });
    return true;
  }

  function startFixtureScenario(text: string, attachments?: AttachmentInput[]) {
    if (!chatHandler) return;
    if (pendingScenario) {
      throw new Error("fixture: a safety scenario is already waiting");
    }
    const { turnId, userId, assistantId } = fixtureIds();
    const id = {
      session_id: session.sessionId,
      thread_id: session.threadId,
      turn_id: turnId,
    };
    const display = displayFixtureText(text, attachments);
    chatHandler({ kind: "user", ...id, message_id: userId, text: display });
    chatHandler({ kind: "assistant_start", ...id, message_id: assistantId });
    const toolId = `tool-${crypto.randomUUID()}`;
    const toolName =
      scenario === "approval" ? "mcp__Haiku__manifest" : "fixture_tool";
    chatHandler({
      kind: "tool_call_start",
      ...id,
      message_id: assistantId,
      name: toolName,
      id: toolId,
    });

    if (scenario === "tool-error") {
      chatHandler({
        kind: "tool_call_result",
        ...id,
        message_id: assistantId,
        name: "fixture_tool",
        id: toolId,
        summary: "[fixture malformed tool output clipped]",
        isError: true,
      });
      chatHandler({
        kind: "text_delta",
        ...id,
        message_id: assistantId,
        text: "Fixture closed the failed tool turn safely.",
      });
      chatHandler({ kind: "done", ...id, message_id: assistantId });
      return;
    }

    if (scenario === "approval") {
      const approvalId = `approval-${crypto.randomUUID()}`;
      pendingScenario = {
        kind: "approval",
        turnId,
        assistantId,
        toolId,
        approvalId,
      };
      chatHandler({
        kind: "approval_needed",
        ...id,
        message_id: assistantId,
        approval_id: approvalId,
        tool_name: "mcp__Haiku__manifest",
        tool_call_id: toolId,
        risk: "exec",
        path: "Haiku · manifest",
        summary: "Run manifest on the Haiku MCP server",
        diff: "{}",
      });
      return;
    }

    if (scenario === "question") {
      const questionId = `question-${crypto.randomUUID()}`;
      pendingScenario = {
        kind: "question",
        turnId,
        assistantId,
        toolId,
        questionId,
      };
      chatHandler({
        kind: "question_needed",
        ...id,
        message_id: assistantId,
        question_id: questionId,
        tool_call_id: toolId,
        prompt: "Which deterministic fixture path should continue?",
        choices: ["safe", "review"],
        multiple: false,
        placeholder: "Choose a path",
      });
      return;
    }

    pendingScenario = { kind: "cancel", turnId, assistantId, toolId };
    cancelTimer = setTimeout(() => {
      if (pendingScenario?.turnId === turnId) finishScenario("cancel");
    }, 5_000);
  }

  return {
    mode: "fixture",
    async listExternalAgents() {
      return [
        {
          id: "claude",
          label: "Claude Code",
          scope: "Project zest.toml",
          mode: "Headless CLI",
          workspace: "Isolated worktree",
          statusLabel: enabledExternalAgents.has("claude") ? "Delegation enabled" : "Delegation off",
          detail: enabledExternalAgents.has("claude")
            ? "Delegates through your Claude Code CLI session."
            : "Enable delegation to let Zest send bounded tasks to Claude Code.",
          configured: enabledExternalAgents.has("claude"),
          mcpAllowed: enabledExternalAgents.has("claude") && fixtureMcpAgents.has("claude"),
          model: fixtureExternalModels.get("claude") ?? "",
          models: fixtureExternalModelOptions.claude,
          preset: true,
        },
        {
          id: "gemini",
          label: "Gemini CLI",
          scope: "Project zest.toml",
          mode: "CLI via ACP",
          workspace: "Isolated worktree",
          statusLabel: enabledExternalAgents.has("gemini") ? "Delegation enabled" : "Delegation off",
          detail: enabledExternalAgents.has("gemini")
            ? "Delegates through your Gemini CLI session."
            : "Enable delegation to let Zest send bounded tasks to Gemini CLI.",
          configured: enabledExternalAgents.has("gemini"),
          mcpAllowed: enabledExternalAgents.has("gemini") && fixtureMcpAgents.has("gemini"),
          model: fixtureExternalModels.get("gemini") ?? "",
          models: fixtureExternalModelOptions.gemini,
          preset: true,
        },
      ];
    },
    async setExternalAgent(id, enabled) {
      if (enabled) enabledExternalAgents.add(id);
      else {
        enabledExternalAgents.delete(id);
        fixtureMcpAgents.delete(id);
        fixtureExternalModels.delete(id);
      }
    },
    async setExternalAgentMcp(id, enabled) {
      if (enabled) fixtureMcpAgents.add(id);
      else fixtureMcpAgents.delete(id);
    },
    async setExternalAgentModel(id, model) {
      if (model?.trim()) fixtureExternalModels.set(id, model.trim());
      else fixtureExternalModels.delete(id);
    },
    async checkExternalAgent() {
      return {
        available: false,
        authenticated: null,
        detail: "CLI checks are unavailable in the fixture.",
      };
    },
    async listMcpServers() {
      return [...fixtureMcpServers.values()];
    },
    async saveMcpServer(input) {
      const id = input.id.trim();
      // The fixture starts no process, so a saved server is honestly
      // "not checked" rather than pretending to have listed tools.
      fixtureMcpServers.set(id, {
        id,
        command: input.command.trim(),
        args: input.args.filter((arg) => arg.trim().length > 0),
        url: (input.url ?? "").trim(),
        headers: input.headers ?? {},
        envVars: input.envVars.filter((name) => name.trim().length > 0),
        enabled: input.enabled,
        timeoutSecs: input.timeoutSecs ?? 120,
        scope: "the fixture config",
        tools: [],
        statusLabel: input.enabled ? "Not checked" : "Off",
        detail: input.enabled
          ? "Check the server to load the tools it offers."
          : "Turn it on to load its tools into new chats.",
      });
      return [...fixtureMcpServers.values()];
    },
    async setMcpServerEnabled(id, enabled) {
      const existing = fixtureMcpServers.get(id);
      if (existing) {
        fixtureMcpServers.set(id, {
          ...existing,
          enabled,
          statusLabel: enabled ? (existing.tools.length ? "Ready" : "Not checked") : "Off",
        });
      }
      return [...fixtureMcpServers.values()];
    },
    async removeMcpServer(id) {
      fixtureMcpServers.delete(id);
      return [...fixtureMcpServers.values()];
    },
    async checkMcpServer() {
      return {
        ok: false,
        detail: "MCP servers cannot be started in the fixture.",
        tools: [],
      };
    },
    async listProviders() {
      return [
        {
          id: "fixture",
          label: "Fixture",
          method: "offline",
          statusKind: "ready",
          statusLabel: "Ready",
          detail: "Deterministic UI stream",
          selectable: true,
          canConnect: false,
          configured: true,
          ownsAgentLoop: false,
          defaultModel: DEFAULT_CODEX_MODEL,
          models: FIXTURE_MODELS,
        },
      ];
    },
    async usageSnapshot() {
      return {
        providers: [
          {
            providerId: "fixture",
            measured: {
              label: "Measured by Zest",
              requests: 812,
              inputTokens: 26_000,
              outputTokens: 18_000,
              cacheWriteTokens: 9_000,
              cacheReadTokens: 947_000,
              totalTokens: 1_000_000,
            },
            headroom: { kind: "not_reported", label: "Not reported" },
          },
        ],
        externalWorkers: [],
      };
    },
    async providerQuota() {
      return {
        checkedAt: Math.floor(Date.now() / 1000),
        providers: [
          {
            providerId: "fixture",
            kind: "unavailable" as const,
            detail: "Live provider checks are unavailable in the fixture.",
            available: null,
            balances: [],
            windows: [],
            plan: null,
            spendLimit: null,
          },
        ],
      };
    },
    async listPlugins() {
      return [
        {
          id: "now-playing",
          name: "Now Playing",
          description: "See and control your music.",
          enabled: fixtureNowPlayingEnabled,
          available: true,
          detail: "Ready",
        },
        {
          id: "wallpaper",
          name: "Wallpaper",
          description: "Use an image as the app background.",
          enabled: fixtureWallpaperEnabled,
          available: true,
          detail: "Ready",
        },
      ];
    },
    async openPluginsFolder() {},
    async setPluginEnabled(id: string, enabled: boolean) {
      if (id === "now-playing") {
        fixtureNowPlayingEnabled = enabled;
        return this.listPlugins();
      }
      if (id === "wallpaper") {
        fixtureWallpaperEnabled = enabled;
        return this.listPlugins();
      }
      throw new Error(`fixture: unknown plugin ${id}`);
    },
    async nowPlaying() {
      return fixtureNowPlayingEnabled
        ? {
            status: fixtureNowPlayingStatus,
            title: "Midnight City",
            artist: "M83",
            album: "Hurry Up, We're Dreaming",
            artworkDataUrl: FIXTURE_ARTWORK_DATA_URL,
            sourceApp: "fixture-player",
            positionSecs: 86,
            durationSecs: 243,
            volumePercent: fixtureNowPlayingVolume,
            canPrevious: true,
            canToggle: true,
            canNext: true,
            detail: "Ready",
            observedAt: Math.floor(Date.now() / 1000),
          }
        : {
            status: "disabled" as const,
            detail: "Turn it on in Settings.",
            observedAt: Math.floor(Date.now() / 1000),
          };
    },
    async controlNowPlaying(action) {
      if (!fixtureNowPlayingEnabled) throw new Error("fixture: Now Playing is disabled");
      if (action === "toggle") {
        fixtureNowPlayingStatus =
          fixtureNowPlayingStatus === "playing" ? "paused" : "playing";
      }
      return this.nowPlaying();
    },
    async setNowPlayingVolume(volumePercent) {
      if (!fixtureNowPlayingEnabled) throw new Error("fixture: Now Playing is disabled");
      fixtureNowPlayingVolume = Math.max(0, Math.min(100, volumePercent));
      return this.nowPlaying();
    },
    async wallpaper() {
      if (!fixtureWallpaperEnabled) {
        return {
          status: "disabled" as const,
          sourceName: null,
          filter: "none" as const,
          imageDataUrl: null,
          detail: "Turn it on in Extras.",
          observedAt: Math.floor(Date.now() / 1000),
        };
      }
      if (!fixtureWallpaperChosen) {
        return {
          status: "empty" as const,
          sourceName: null,
          filter: fixtureWallpaperFilter,
          imageDataUrl: null,
          detail: "Choose an image.",
          observedAt: Math.floor(Date.now() / 1000),
        };
      }
      return {
        status: "ready" as const,
        sourceName: "fixture.png",
        filter: fixtureWallpaperFilter,
        imageDataUrl: FIXTURE_WALLPAPER_PREVIEWS[fixtureWallpaperFilter],
        detail: "Ready",
        observedAt: Math.floor(Date.now() / 1000),
      };
    },
    async pickWallpaper() {
      if (!fixtureWallpaperEnabled) throw new Error("fixture: Wallpaper is disabled");
      fixtureWallpaperChosen = true;
      return this.wallpaper();
    },
    async setWallpaperFilter(filter) {
      if (!fixtureWallpaperEnabled) throw new Error("fixture: Wallpaper is disabled");
      if (!fixtureWallpaperChosen) throw new Error("Choose an image first.");
      fixtureWallpaperFilter = filter;
      return this.wallpaper();
    },
    async clearWallpaper() {
      if (!fixtureWallpaperEnabled) throw new Error("fixture: Wallpaper is disabled");
      fixtureWallpaperChosen = false;
      fixtureWallpaperFilter = "none";
      return this.wallpaper();
    },
    async usageReport(days: number) {
      // Shaped to exercise the parts of the screen that only appear when
      // something is imperfect: two providers on the chart, a model with no
      // rate, and tokens from before per-model metering. A fixture where
      // everything is priced would leave the coverage card untested offline.
      const day = 86_400_000;
      const midnight = new Date();
      midnight.setHours(0, 0, 0, 0);
      const iso = (offset: number) =>
        new Date(midnight.getTime() - offset * day).toLocaleDateString("en-CA");

      const series = Array.from({ length: days }, (_, index) => {
        const back = days - 1 - index;
        // A calm baseline with two bursts, so the area chart has a shape to
        // draw rather than a flat line.
        const wave = Math.sin(index / 3) * 0.4 + 1;
        const burst = back < 3 ? 3.2 : back < 9 ? 1.6 : 1;
        const quiet = back % 11 === 0;
        const costUsd = quiet ? 0 : Number((wave * burst * 4.15).toFixed(2));
        const tokens = Math.round(costUsd * 1_180_000);
        return {
          date: iso(back),
          costUsd,
          tokens,
          requests: quiet ? 0 : Math.round(costUsd * 2),
          byProvider: quiet
            ? []
            : [
                {
                  providerId: "codex",
                  costUsd: Number((costUsd * 0.61).toFixed(2)),
                  tokens: Math.round(tokens * 0.71),
                },
                {
                  providerId: "anthropic",
                  costUsd: Number((costUsd * 0.39).toFixed(2)),
                  tokens: Math.round(tokens * 0.29),
                },
              ],
        };
      });

      const costUsd = series.reduce((sum, point) => sum + point.costUsd, 0);
      const tokens = series.reduce((sum, point) => sum + point.tokens, 0);
      const activeDays = series.filter((point) => point.requests > 0).length;
      const cacheSavingsUsd = costUsd * 5.8;

      return {
        days,
        startDate: series[0]?.date ?? iso(0),
        endDate: series[series.length - 1]?.date ?? iso(0),
        totals: {
          costUsd,
          requests: series.reduce((sum, point) => sum + point.requests, 0),
          processedTokens: tokens,
          uncachedInputTokens: Math.round(tokens * 0.026),
          cachedInputTokens: Math.round(tokens * 0.947),
          cacheWriteTokens: Math.round(tokens * 0.009),
          outputTokens: Math.round(tokens * 0.018),
          cacheSavingsUsd,
          activeDays,
          tokensPerActiveDay: activeDays ? Math.round(tokens / activeDays) : 0,
          servedFromCachePercent: 96.4,
          writtenToCachePercent: 0.9,
          readFreshPercent: 2.7,
          cacheReuseRatio: 105.2,
          cacheHitPercent: 96.4,
          unattributedTokens: Math.round(tokens * 0.01),
        },
        series,
        providers: [
          {
            providerId: "codex",
            costUsd: costUsd * 0.61,
            tokens: Math.round(tokens * 0.71),
            sharePercent: 61,
          },
          {
            providerId: "anthropic",
            costUsd: costUsd * 0.39,
            tokens: Math.round(tokens * 0.29),
            sharePercent: 39,
          },
        ],
        models: [
          {
            providerId: "codex",
            modelId: "gpt-5.6-sol",
            costUsd: costUsd * 0.61,
            costSource: "modelPriced" as const,
            sharePercent: 61,
            requests: 812,
            tokens: Math.round(tokens * 0.71),
            inputTokens: Math.round(tokens * 0.02),
            outputTokens: Math.round(tokens * 0.012),
            cacheWriteTokens: Math.round(tokens * 0.006),
            cacheReadTokens: Math.round(tokens * 0.672),
          },
          {
            providerId: "anthropic",
            modelId: "claude-sonnet-4-6",
            costUsd: costUsd * 0.39,
            costSource: "providerReported" as const,
            sharePercent: 39,
            requests: 344,
            tokens: Math.round(tokens * 0.28),
            inputTokens: Math.round(tokens * 0.006),
            outputTokens: Math.round(tokens * 0.006),
            cacheWriteTokens: Math.round(tokens * 0.003),
            cacheReadTokens: Math.round(tokens * 0.265),
          },
          {
            providerId: "codex",
            modelId: "gpt-5.6-terra",
            costUsd: null,
            costSource: "unpriced" as const,
            sharePercent: 0,
            requests: 12,
            tokens: Math.round(tokens * 0.01),
            inputTokens: Math.round(tokens * 0.004),
            outputTokens: Math.round(tokens * 0.002),
            cacheWriteTokens: 0,
            cacheReadTokens: Math.round(tokens * 0.004),
          },
        ],
        quality: {
          providerReportedPercent: 12,
          pricedPercent: 86,
          unpricedPercent: 1,
          unattributedPercent: 1,
          unpricedModels: ["gpt-5.6-terra"],
          cacheSavingsUsd,
          savingsMultiple: 5.8,
        },
        scan: {
          filesScanned: 3,
          filesCached: 146,
          filesSkipped: 12,
          filesFailed: 0,
          records: 44_325,
          duplicatesDropped: 515,
          roots: [
            { providerId: "claude-cli", path: "/fixture/.claude/projects", exists: true },
            { providerId: "codex-cli", path: "/fixture/.codex/sessions", exists: true },
          ],
        },
        // One worker that reported a cost and one that stayed silent, so both
        // the measured-cost row and the "Not reported" case are visible offline.
        externalWorkers: [
          {
            workerId: "claude",
            invocations: 3,
            usageReports: 3,
            tokenReports: 3,
            inputTokens: 30,
            outputTokens: 2213,
            thoughtTokens: null,
            cachedReadTokens: 593_420,
            cachedWriteTokens: 39_235,
            reportedTokenTotal: 634_898,
            contextUsed: null,
            contextSize: null,
            lastCost: { amount: "0.1823282", currency: "USD" },
            lastSeen: 0,
          },
          {
            workerId: "gemini",
            invocations: 1,
            usageReports: 0,
            tokenReports: 0,
            inputTokens: null,
            outputTokens: null,
            thoughtTokens: null,
            cachedReadTokens: null,
            cachedWriteTokens: null,
            reportedTokenTotal: null,
            contextUsed: null,
            contextSize: null,
            lastCost: null,
            lastSeen: 0,
          },
        ],
        pricesPath: "/fixture/prices.toml",
        rates: {
          catalogModels: 1579,
          overrides: 0,
          fetchedAt: Math.floor(Date.now() / 1000) - 3 * 3600,
          stale: false,
          sourceUrl: "https://example.invalid/model_prices.json",
        },
      };
    },
    async openPricesFile() {
      notAvailable("openPricesFile");
    },
    async refreshRates() {
      // The fixture never reaches the network; report the same figures the
      // canned report already carries so the two cannot disagree on screen.
      return {
        catalogModels: 1579,
        overrides: 0,
        fetchedAt: Math.floor(Date.now() / 1000) - 3 * 3600,
        stale: false,
        sourceUrl: "https://example.invalid/model_prices.json",
      };
    },
    async setProviderKey() {
      notAvailable("setProviderKey");
    },
    async deleteProviderKey() {
      notAvailable("deleteProviderKey");
    },
    async providerKeyPresent() {
      return false;
    },
    async configureApiProvider() {
      notAvailable("configureApiProvider");
    },
    async configureAnthropicProvider() {
      notAvailable("configureAnthropicProvider");
    },
    async configureClaudeCodeProvider() {
      notAvailable("configureClaudeCodeProvider");
    },
    async configureCodexCliProvider() {
      notAvailable("configureCodexCliProvider");
    },
    async configureCodexOAuthProvider() {
      notAvailable("configureCodexOAuthProvider");
    },
    async codexCliAvailable() {
      return true;
    },
    async openProjectConfig() {
      notAvailable("openProjectConfig");
    },
    async profileStats() {
      // Enough shape to exercise the heatmap offline: a long run of days, a
      // gap, and a metering start part-way through so the "no token data"
      // rendering is visible rather than only reachable on a real install.
      const day = 86_400;
      const today = Math.floor(Date.now() / 1000);
      const iso = (offsetDays: number) =>
        new Date((today - offsetDays * day) * 1000).toISOString().slice(0, 10);

      const days = [];
      for (let back = 180; back >= 0; back--) {
        // A believable rhythm rather than noise: quiet weekends, busy weekdays.
        const weekday = new Date((today - back * day) * 1000).getDay();
        const busy = weekday !== 0 && weekday !== 6;
        const chats = busy ? (back % 5 === 0 ? 0 : 1 + (back % 4)) : back % 3 === 0 ? 1 : 0;
        if (chats === 0 && back % 7 !== 0) continue;
        days.push({
          date: iso(back),
          chats,
          messages: chats * (4 + (back % 9)),
          // Metering began 90 days ago; earlier cells carry no token figure.
          ...(back <= 90
            ? { tokens: chats * 12_000 + (back % 11) * 900, requests: chats * 3 }
            : {}),
        });
      }

      return {
        totalChats: days.reduce((sum, d) => sum + d.chats, 0),
        totalMessages: days.reduce((sum, d) => sum + d.messages, 0),
        totalTokens: 5_252_800_000,
        totalRequests: 12_480,
        peakDayTokens: 338_300_000,
        longestChatSecs: 4_380,
        currentStreakDays: 28,
        longestStreakDays: 71,
        firstActivity: today - 180 * day,
        days,
        meteringSince: iso(90),
      };
    },
    async setLocalOffset() {
      /* fixture: no core to inform */
    },
    async lastProvider() {
      return "fixture";
    },
    async startLogin() {
      return notAvailable("startLogin");
    },
    async loginStatus() {
      return { state: "idle", detail: null };
    },
    async cancelLogin() {
      /* fixture: no child process to stop */
    },
    async startSession() {
      clearFixtureScenario();
      fixturePinned = false;
      session = { ...FIXTURE_SESSION, messages: [] };
      return { ...session };
    },
    async switchSessionProvider(providerId, model) {
      session = {
        ...session,
        provider: providerId,
        label: providerId,
        model: model ?? session.model,
        warning: undefined,
      };
      return { ...session };
    },
    async updateSessionOptions(options) {
      session = {
        ...session,
        model: options.model ?? session.model,
        effort: options.effort ?? session.effort,
      };
      // Metadata only, matching Rust: options do not touch the transcript, so
      // the reply does not carry one.
      const { messages: _messages, ...meta } = session;
      return meta;
    },
    async resetSessionOptions() {
      session = {
        ...session,
        model: session.defaultModel ?? DEFAULT_CODEX_MODEL,
        effort: DEFAULT_EFFORT,
      };
      const { messages: _messages, ...meta } = session;
      return meta;
    },
    async listThreads() {
      const threads: ThreadSummary[] = [];
      // Seeded rows stay visible. A freshly opened empty draft does not — the
      // desktop store also waits for the first user message before writing one.
      if (
        session.messages.length > 0 ||
        fixtureThreadTitles.has(session.threadId)
      ) {
        threads.push({
          id: session.threadId,
          createdAt: 0,
          updatedAt: Math.floor(Date.now() / 1000),
          title: fixtureThreadTitles.get(session.threadId) || "Fixture",
          pinned: fixturePinned,
          providerId: "codex",
          messageCount:
            fixtureTranscripts.get(session.threadId)?.length ??
            session.messages.length,
        });
      }
      // A provider Zest has no mark for, so the generic fallback is visible
      // offline. This is the ordinary case for a local model, and it is the
      // half of the mapping most likely to regress unnoticed.
      if (session.threadId !== "fixture-local") {
        threads.push({
          id: "fixture-local",
          createdAt: Math.floor(Date.now() / 1000) - 3600,
          updatedAt: Math.floor(Date.now() / 1000) - 3600,
          title: fixtureThreadTitles.get("fixture-local") || "Local model chat",
          pinned: false,
          providerId: "ollama",
          messageCount: 1,
        });
      }
      if (session.threadId !== LONG_THREAD_ID) {
        threads.push({
          id: LONG_THREAD_ID,
          createdAt: Math.floor(Date.now() / 1000) - 7200,
          updatedAt: Math.floor(Date.now() / 1000) - 7200,
          title: fixtureThreadTitles.get(LONG_THREAD_ID) || "Fifteen turns",
          pinned: false,
          providerId: "codex",
          messageCount: fixtureTranscripts.get(LONG_THREAD_ID)?.length ?? 30,
        });
      }
      return threads;
    },
    async forgetWorkspace(projectPath) {
      if (projectPath !== workspace) throw new Error("fixture: unknown workspace");
      throw new Error("Switch to another project before removing the active workspace.");
    },
    async listChatProjects() {
      const threads = await this.listThreads();
      return [
        {
          name: workspace.split(/[/\\]/).filter(Boolean).pop() || "fixture",
          path: workspace,
          active: !session.isFreeChat,
          threads: session.isFreeChat ? [] : threads,
        },
        {
          name: "Free chats",
          path: null,
          active: session.isFreeChat,
          threads: session.isFreeChat
            ? threads
            : [
                {
                  id: "fixture-free",
                  createdAt: Math.floor(Date.now() / 1000) - 1800,
                  updatedAt: Math.floor(Date.now() / 1000) - 1800,
                  title: fixtureThreadTitles.get("fixture-free") || "Free chat",
                  pinned: false,
                  providerId: "codex",
                  messageCount: 1,
                },
              ],
        },
      ];
    },
    async searchChats(query) {
      const needle = query.trim();
      if (!needle) return [];
      const projects = await this.listChatProjects();
      const hits: ChatSearchHit[] = [];
      for (const project of projects) {
        for (const thread of project.threads) {
          const title = thread.title?.trim() || "Untitled chat";
          const liveText =
            thread.id === session.threadId
              ? session.messages.map((message) => message.text).join("\n")
              : "";
          const storedText =
            fixtureTranscripts.get(thread.id)
              ?.map((message) => message.text)
              .join("\n") ?? "";
          const transcript = [
            liveText,
            storedText,
            fixtureThreadBodies.get(thread.id) ?? "",
          ]
            .filter(Boolean)
            .join("\n");
          const snippet = matchExcerpt(transcript, needle);
          const titleHit = title.toLowerCase().includes(needle.toLowerCase());
          if (!snippet && !titleHit) continue;
          const matched = (
            thread.id === session.threadId ? session.messages : []
          )
            .concat(fixtureTranscripts.get(thread.id) ?? [])
            .find((message) => matchExcerpt(message.text, needle));
          hits.push({
            id: thread.id,
            title,
            projectName: project.path === null ? "No workspace" : project.name,
            projectPath: project.path,
            updatedAt: thread.updatedAt,
            snippet,
            messageId: matched?.id,
          });
        }
      }
      return hits.sort((a, b) => b.updatedAt - a.updatedAt).slice(0, 24);
    },
    async openProjectChat(options) {
      const targetRoot = options.root;
      const openingFreeChat = targetRoot === null;
      if (targetRoot !== null) {
        workspace = targetRoot;
      }
      if (options.newThread) {
        fixturePinned = false;
        session = {
          ...session,
          root: openingFreeChat ? "." : workspace,
          isFreeChat: openingFreeChat,
          threadId: `fixture-${crypto.randomUUID()}`,
          messages: [],
          hasOlderMessages: false,
          hasNewerMessages: false,
          hiddenUserTurns: 0,
        };
        return { ...session };
      }
      const nextId = options.threadId || session.threadId;
      if (fixtureTranscripts.has(nextId)) {
        session = windowSessionFor(
          nextId,
          {
            root: openingFreeChat ? "." : workspace,
            isFreeChat: openingFreeChat,
          },
          options.focusMessageId
        );
        return { ...session };
      }
      session = {
        ...session,
        root: openingFreeChat ? "." : workspace,
        isFreeChat: openingFreeChat,
        threadId: nextId,
      };
      return { ...session };
    },
    async loadThread(id: string) {
      if (fixtureTranscripts.has(id)) {
        session = windowSessionFor(id);
        return { ...session };
      }
      if (id !== session.threadId) {
        throw new Error(`fixture: unknown thread ${id}`);
      }
      return { ...session };
    },
    async newThread() {
      clearFixtureScenario();
      fixturePinned = false;
      session = {
        ...FIXTURE_SESSION,
        root: workspace,
        isFreeChat: false,
        threadId: `fixture-${crypto.randomUUID()}`,
        messages: [],
      };
      return { ...session };
    },
    async loadOlderThreadMessages(options: {
      threadId: string;
      beforeMessageId: string;
    }): Promise<OlderThreadMessages> {
      if (session.threadId !== options.threadId) {
        throw new Error("fixture: that chat is not open");
      }
      const full = fixtureTranscripts.get(options.threadId) ?? session.messages;
      const page = olderUserTurns(
        full,
        options.beforeMessageId,
        THREAD_OLDER_USER_TURNS
      );
      return {
        threadId: options.threadId,
        messages: page.messages,
        hasOlderMessages: page.hasOlder,
        hasNewerMessages: page.hasNewer,
        hiddenUserTurns: page.hiddenUserTurns,
      };
    },
    async loadNewerThreadMessages(options: {
      threadId: string;
      afterMessageId: string;
    }): Promise<OlderThreadMessages> {
      if (session.threadId !== options.threadId) {
        throw new Error("fixture: that chat is not open");
      }
      const full = fixtureTranscripts.get(options.threadId) ?? session.messages;
      const page = newerUserTurns(
        full,
        options.afterMessageId,
        THREAD_OLDER_USER_TURNS
      );
      return {
        threadId: options.threadId,
        messages: page.messages,
        hasOlderMessages: page.hasOlder,
        hasNewerMessages: page.hasNewer,
        hiddenUserTurns: page.hiddenUserTurns,
      };
    },
    async sessionInfo() {
      return { ...session };
    },
    async forkThread() {
      fixturePinned = false;
      session = {
        ...session,
        threadId: `fixture-${crypto.randomUUID()}`,
        checkpoints: [],
      };
      return { ...session };
    },
    async forkThreadFromCheckpoint(checkpointId: string) {
      const checkpointIndex = session.checkpoints.findIndex(
        (item) => item.id === checkpointId
      );
      const checkpoint =
        checkpointIndex >= 0 ? session.checkpoints[checkpointIndex] : undefined;
      if (!checkpoint) throw new Error("fixture: checkpoint not found");
      fixturePinned = false;
      session = {
        ...session,
        threadId: `fixture-${crypto.randomUUID()}`,
        checkpoints: session.checkpoints.slice(0, checkpointIndex + 1),
        messages: session.messages.slice(0, checkpoint.messageCount),
      };
      return { ...session };
    },
    async rewindThread(checkpointId: string) {
      const checkpointIndex = session.checkpoints.findIndex(
        (item) => item.id === checkpointId
      );
      const checkpoint =
        checkpointIndex >= 0 ? session.checkpoints[checkpointIndex] : undefined;
      if (!checkpoint) throw new Error("fixture: checkpoint not found");
      session = {
        ...session,
        checkpoints: session.checkpoints.slice(0, checkpointIndex + 1),
        messages: session.messages.slice(0, checkpoint.messageCount),
      };
      return { ...session };
    },
    async editMessage(messageId: string) {
      const index = session.messages.findIndex((message) => message.id === messageId);
      if (index < 0 || session.messages[index]?.role !== "user") {
        throw new Error("fixture: user message not found");
      }
      session = { ...session, messages: session.messages.slice(0, index) };
      return { ...session };
    },
    async compactContext() {
      return {
        usage: await this.contextUsage(),
        prunedOnly: false,
        resultsPruned: 0,
      };
    },
    async deleteThread(id: string) {
      if (id === session.threadId) {
        return this.newThread();
      }
      return { ...session };
    },
    async setThreadPinned(_id, _projectPath, pinned) {
      fixturePinned = pinned;
    },
    async renameThread(id, _projectPath, title) {
      const normalized = title.trim();
      if (!normalized) throw new Error("fixture: chat title is empty");
      if ([...normalized].length > MAX_FIXTURE_THREAD_TITLE_CHARS) {
        throw new Error("fixture: chat title is too long");
      }
      if (
        id !== session.threadId &&
        id !== "fixture-local" &&
        id !== LONG_THREAD_ID
      ) {
        throw new Error(`fixture: unknown thread ${id}`);
      }
      fixtureThreadTitles.set(id, normalized);
      const summary = (await this.listThreads()).find((thread) => thread.id === id);
      if (!summary) throw new Error(`fixture: unknown thread ${id}`);
      return summary;
    },
    async sendMessage(
      text: string,
      attachments?: AttachmentInput[],
      target?: InputTarget,
    ) {
      if (target === "followup" || target === "steer" || target === "inject") {
        const input = {
          id: `input-${crypto.randomUUID()}`,
          target,
          text,
          createdAt: Date.now(),
          attachments: (attachments ?? []).map((attachment) => ({
            name: attachment.name,
            detail: attachment.detail,
            content: attachment.content ?? null,
            status: attachment.status,
            ...(attachment.kind ? { kind: attachment.kind } : {}),
            ...(attachment.mediaType ? { mediaType: attachment.mediaType } : {}),
            ...(attachment.dataBase64 ? { dataBase64: attachment.dataBase64 } : {}),
          })),
        };
        session = {
          ...session,
          pendingInputs: [...session.pendingInputs, input],
        };
        chatHandler?.({
          kind: "input_queued",
          session_id: session.sessionId,
          thread_id: session.threadId,
          input,
        });
        return;
      }
      if (scenario) {
        startFixtureScenario(text, attachments);
        return;
      }
      emitFixtureEcho(text, attachments);
    },
    async updateQueuedInput(threadId: string, inputId: string, text: string) {
      if (threadId !== session.threadId) throw new Error("fixture: unknown thread");
      const input = session.pendingInputs.find((candidate) => candidate.id === inputId);
      if (!input) throw new Error("fixture: queued input not found");
      session = {
        ...session,
        pendingInputs: session.pendingInputs.map((candidate) =>
          candidate.id === inputId ? { ...candidate, text } : candidate,
        ),
      };
      chatHandler?.({
        kind: "input_updated",
        session_id: session.sessionId,
        thread_id: session.threadId,
        input_id: inputId,
        text,
      });
    },
    async removeQueuedInput(threadId: string, inputId: string) {
      if (threadId !== session.threadId) throw new Error("fixture: unknown thread");
      if (!session.pendingInputs.some((candidate) => candidate.id === inputId)) {
        throw new Error("fixture: queued input not found");
      }
      session = {
        ...session,
        pendingInputs: session.pendingInputs.filter((candidate) => candidate.id !== inputId),
      };
      chatHandler?.({
        kind: "input_removed",
        session_id: session.sessionId,
        thread_id: session.threadId,
        input_id: inputId,
      });
    },
    async resumeQueuedInputs(threadId: string) {
      if (threadId !== session.threadId) throw new Error("fixture: unknown thread");
      const input = session.pendingInputs.find((candidate) => candidate.target === "followup");
      if (!input) throw new Error("fixture: no resumable queued messages");
      session = {
        ...session,
        pendingInputs: session.pendingInputs.filter((candidate) => candidate.id !== input.id),
      };
      chatHandler?.({
        kind: "input_removed",
        session_id: session.sessionId,
        thread_id: session.threadId,
        input_id: input.id,
      });
      if (scenario) {
        startFixtureScenario(input.text, input.attachments);
      } else {
        emitFixtureEcho(input.text, input.attachments);
      }
    },
    async listJobs(_threadId?: string): Promise<JobSnapshot[]> {
      return [];
    },
    async jobOutput(_jobId: string, _options?): Promise<JobRead> {
      throw new Error("fixture: jobs are not available");
    },
    async jobKill(_jobId: string, _reason?: string, _threadId?: string): Promise<JobSnapshot> {
      throw new Error("fixture: jobs are not available");
    },
    async saveMarkdown(suggestedName, markdown) {
      const filename = safeMarkdownFilename(suggestedName, "response");
      const blob = new Blob([markdown], { type: "text/markdown;charset=utf-8" });
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = filename;
      link.click();
      URL.revokeObjectURL(url);
      return filename;
    },
    async cancelTurn() {
      if (pendingScenario?.kind === "cancel") {
        finishScenario("cancel");
      }
    },
    async resolveApproval(approvalId: string, decision) {
      if (
        !pendingScenario ||
        pendingScenario.kind !== "approval" ||
        pendingScenario.approvalId !== approvalId
      ) {
        throw new Error("fixture: no pending approval");
      }
      if (decision === "deny") {
        finishScenario("deny");
      } else {
        finishScenario("allow");
      }
    },
    async resolveQuestion(questionId: string) {
      if (
        !pendingScenario ||
        pendingScenario.kind !== "question" ||
        pendingScenario.questionId !== questionId
      ) {
        throw new Error("fixture: no pending question");
      }
      finishScenario("answer");
    },
    async setApprovalMode(mode: ApprovalMode) {
      return mode;
    },
    async approvalMode() {
      return "auto";
    },
    async verifyProvider() {
      /* fixture: nothing to verify */
    },
    async listCommands() {
      return [
        {
          name: "model",
          description: "Switch model or provider",
          kind: "builtin" as const,
        },
        ...[...fixtureMcpServers.values()]
          .filter((server) => server.enabled)
          .map((server) => ({
            name: server.id,
            description: `Use the ${server.id} MCP server`,
            kind: "mcp" as const,
          })),
      ];
    },
    async endSession() {
      /* no-op */
    },
    async getSystemPrompt() {
      return {
        base: "Fixture base system prompt.",
        custom: "",
        composedPreview: "Fixture base system prompt.",
        customPath: ".zest/system.md",
      };
    },
    async setSystemPrompt(custom: string) {
      return {
        base: "Fixture base system prompt.",
        custom,
        composedPreview: custom
          ? `Fixture base system prompt.\n\n# Project instructions\n\n${custom}`
          : "Fixture base system prompt.",
        customPath: ".zest/system.md",
      };
    },
    async listSkills() {
      return [];
    },
    async getWorkspaceFolder() {
      return workspace;
    },
    async revealWorkspaceFolder() {
      notAvailable("reveal the project folder");
    },
    async listWorkspaceFiles(relativePath?: string | null) {
      const normalized = relativePath?.replace(/\\/g, "/").replace(/^\/+|\/+$/g, "") ?? "";
      if (normalized === "src") {
        return [
          {
            path: "src/main.ts",
            name: "main.ts",
            kind: "file" as const,
            size: 520,
            modifiedAt: 1_700_000_000,
          },
          {
            path: "src/lib.ts",
            name: "lib.ts",
            kind: "file" as const,
            size: 340,
            modifiedAt: 1_700_000_000,
          },
        ];
      }
      if (normalized) return [];
      return [
        {
          path: "src",
          name: "src",
          kind: "directory" as const,
          size: null,
          modifiedAt: null,
        },
        {
          path: "README.md",
          name: "README.md",
          kind: "file" as const,
          size: 1240,
          modifiedAt: 1_700_000_000,
        },
        {
          path: "Cargo.toml",
          name: "Cargo.toml",
          kind: "file" as const,
          size: 860,
          modifiedAt: 1_700_000_000,
        },
      ];
    },
    async readWorkspaceFile(relativePath: string) {
      const content = relativePath.endsWith("README.md")
        ? "# Fixture project\n\nThis is a safe file preview from the offline backend."
        : "[fixture preview]";
      return {
        path: relativePath,
        content,
        truncated: false,
        byteCount: new TextEncoder().encode(content).length,
      };
    },
    async pickWorkspaceFolder() {
      workspace = `fixture/project-${crypto.randomUUID().slice(0, 8)}`;
      session = { ...session, root: workspace, messages: [] };
      return { path: workspace, sessionEnded: false };
    },
    async pickFiles() {
      return [
        {
          id: `att-${crypto.randomUUID()}`,
          name: "sample.pdf",
          path: `${workspace}/sample.pdf`,
          kind: "pdf",
          status: "done",
          detail: "TextBased, 1 pages",
          content: "# Fixture PDF\n\nExtracted markdown from pdf-inspector path.",
        },
      ];
    },
    async preparePastedImage(options) {
      return {
        id: `att-${crypto.randomUUID()}`,
        name: options.name ?? "paste.png",
        path: "clipboard",
        kind: "image",
        status: "done",
        detail: "pasted",
        mediaType: options.mediaType,
        dataBase64: options.dataBase64.includes(",")
          ? options.dataBase64.split(",").pop()!
          : options.dataBase64,
      };
    },
    async gitBranch() {
      return "master";
    },
    async gitContext(): Promise<GitContext> {
      return {
        branch: "master",
        baseBranch: "master",
        branchChanged: false,
        additions: 12,
        deletions: 3,
        changedFiles: 1,
        statsSource: "branch",
      };
    },
    async workspaceChanges(): Promise<WorkspaceChange> {
      return {
        changeId: "fixture-clean",
        repository: "git",
        baseCommit: undefined,
        baseBranch: "master",
        branch: "master",
        changedFiles: [],
        additions: 0,
        deletions: 0,
        diff: "",
        truncated: false,
        unavailable: false,
      };
    },
    async verifyWorkspace() {
      return {
        summary: "Fixture workspace is ready.",
        repository: "git",
        changedFiles: ["src/example.ts"],
        changedFileCount: 1,
        patchCheck: "clean",
      };
    },
    async contextUsage() {
      return {
        usedTokens: 12000,
        windowTokens: 256000,
        remainingTokens: 244000,
        percentFull: 4.7,
        source: "last_turn",
        systemTokens: 3200,
        conversationTokens: 8800,
        // A measured turn, so the three columns sum to usedTokens. Cache-heavy on
        // purpose: that is what a real session looks like, and it is the shape
        // that used to read as an almost-empty window.
        inputTokens: 400,
        cacheReadTokens: 11200,
        cacheWriteTokens: 400,
        messageCount: session.messages.length,
        checkpointCount: session.checkpoints.length,
        canCompact: session.messages.length >= 4,
        autoCompactThresholdPercent: 80,
        shouldAutoCompact: false,
      };
    },
    async getUserProfile() {
      return { displayName: "Fixture", avatarDataUrl: "" };
    },
    async setUserProfile(profile) {
      return profile;
    },
    async onChatEvent(handler) {
      const generation = ++chatHandlerGeneration;
      chatHandler = handler;
      return () => {
        // React Strict Mode can overlap an old async subscription cleanup
        // with a newer registration of the same function. Only the latest
        // registration is allowed to clear the fixture event sink.
        if (chatHandlerGeneration === generation && chatHandler === handler) {
          chatHandler = null;
        }
      };
    },
    async listDelegationJobs(): Promise<DelegationJob[]> {
      return [delegationSnapshot()];
    },
    async listDelegationTargets(): Promise<DelegationTargetOptionView[]> {
      return [
        {
          target: { kind: "provider", providerId: "fixture", model: null, effort: null },
          available: true,
          label: "Fixture provider",
          error: null,
        },
        {
          target: { kind: "externalAgent", agentId: "claude" },
          available: false,
          label: "Claude Code",
          error: "Unavailable in the fixture. Reconnect or choose another target.",
        },
      ];
    },
    async createDelegationJob(request: DelegationCreateInput): Promise<DelegationJob> {
      fixtureDelegationJob = {
        ...fixtureDelegationJob,
        jobId: `fixture-delegation-${Date.now()}`,
        cardId: `fixture-card-${Date.now()}`,
        parentThreadId: request.parentThreadId,
        title: request.title,
        objective: request.objective,
        lane: request.lane,
        scope: request.scope,
        context: request.context ?? [],
        dependsOn: request.dependsOn ?? [],
        workerTarget: request.worker,
        reviewerTarget: request.reviewer ?? { kind: "sameAsWorker" },
        agent: request.worker.kind === "provider" ? request.worker.providerId : request.worker.agentId,
        reviewerAgent: request.worker.kind === "provider" ? request.worker.providerId : request.worker.agentId,
        approved: false,
        status: "awaiting_approval",
        changedFiles: [],
        changedFileCount: 0,
        acceptanceChecks: (request.acceptanceChecks ?? []).map((command: string) => ({ command, status: "pending", output: "" })),
        updatedAt: Date.now(),
      };
      return delegationSnapshot();
    },
    async updateDelegationJob(request: DelegationUpdateInput): Promise<DelegationJob> {
      if (request.jobId !== fixtureDelegationJob.jobId) throw new Error("fixture backend: delegation job was not found");
      fixtureDelegationJob = {
        ...fixtureDelegationJob,
        ...(request.title != null ? { title: request.title } : {}),
        ...(request.objective != null ? { objective: request.objective } : {}),
        ...(request.scope ? { scope: request.scope } : {}),
        ...(request.context ? { context: request.context } : {}),
        ...(request.acceptanceChecks ? { acceptanceChecks: request.acceptanceChecks.map((command: string) => ({ command, status: "pending", output: "" })) } : {}),
        ...(request.worker ? { workerTarget: request.worker } : {}),
        ...(request.reviewer ? { reviewerTarget: request.reviewer } : {}),
        status: "awaiting_approval",
        approved: false,
        updatedAt: fixtureDelegationJob.updatedAt + 1,
      };
      return delegationSnapshot();
    },
    async approveDelegationJob(jobId: string): Promise<DelegationJob> {
      return runFixtureDelegation(jobId);
    },
    async prepareDelegationHandoff(jobId: string) {
      if (jobId !== fixtureDelegationJob.jobId) throw new Error("fixture backend: delegation job was not found");
      return {
        jobId,
        summary: fixtureDelegationJob.workerSummary ?? "No worker summary is available yet.",
        changedFiles: [...fixtureDelegationJob.changedFiles],
        artifactNames: ["worker.diff", "worker-result.json", "review-result.json"],
        status: fixtureDelegationJob.status,
      };
    },
    async getDelegationJob(jobId: string): Promise<DelegationJob> {
      if (jobId !== fixtureDelegationJob.jobId) {
        throw new Error(`fixture backend: delegation job ${jobId} was not found`);
      }
      return delegationSnapshot();
    },
    async cancelDelegationJob(jobId: string): Promise<DelegationJob> {
      if (jobId !== fixtureDelegationJob.jobId) {
        throw new Error(`fixture backend: delegation job ${jobId} was not found`);
      }
      if (!["accepted", "cancelled", "failed"].includes(fixtureDelegationJob.status)) {
        updateDelegation("cancelled", "cancelled", { error: "Cancelled in the fixture." });
      }
      return delegationSnapshot();
    },
    async retryDelegationJob(jobId: string): Promise<DelegationJob> {
      if (jobId !== fixtureDelegationJob.jobId) throw new Error("fixture backend: delegation job was not found");
      updateDelegation("awaiting_approval", "approval_required", { approved: false, error: undefined });
      return delegationSnapshot();
    },
    async applyDelegationJob(jobId: string): Promise<DelegationJob> {
      if (jobId !== fixtureDelegationJob.jobId) {
        throw new Error(`fixture backend: delegation job ${jobId} was not found`);
      }
      if (fixtureDelegationJob.status !== "ready_to_apply") {
        throw new Error("fixture backend: only a ready delegation can be applied");
      }
      updateDelegation("accepted", "applied", { error: undefined });
      return delegationSnapshot();
    },
    async onDelegationEvent(handler: (event: DelegationEvent) => void) {
      const generation = ++delegationHandlerGeneration;
      delegationHandler = handler;
      return () => {
        if (delegationHandlerGeneration === generation && delegationHandler === handler) {
          delegationHandler = null;
        }
      };
    },
    async boot(handler) {
      chatHandlerGeneration += 1;
      chatHandler = handler;
      await runFixtureStream(handler);
    },
  };
}
