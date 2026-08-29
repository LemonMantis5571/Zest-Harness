import type { UnlistenFn } from "@tauri-apps/api/event";

import * as tauriApi from "./api";
import { createFixtureBackend } from "./fixtureBackend";
import type { SkillSummary, SystemPromptInfo } from "./api";
import type {
  ApprovalChoice,
  ApprovalMode,
  CommandView,
  AttachmentInput,
  ChatEvent,
  DelegationEvent,
  DelegationJob,
  DelegationCreateInput,
  DelegationHandoffView,
  DelegationTargetOptionView,
  DelegationUpdateInput,
  CompactionResult,
  ContextUsage,
  ExternalAgentCheck,
  ExternalAgentRow,
  GitContext,
  InputTarget,
  JobRead,
  JobSnapshot,
  LoginStarted,
  LoginStatus,
  McpCheck,
  McpServerRow,
  NowPlayingView,
  PreparedAttachment,
  PluginView,
  ProfileStats,
  ProjectChats,
  ChatSearchHit,
  ProviderRow,
  RatesStatus,
  SessionInfo,
  SessionMeta,
  ThreadSummary,
  UsageReport,
  UsageSnapshot,
  ProviderQuotaSnapshot,
  UserProfile,
  WallpaperFilterId,
  WallpaperView,
  WorkspaceFileContent,
  WorkspaceFileView,
  WorkspacePickResult,
  WorkspaceChange,
  WorkspaceReview,
} from "./types";

export type { SkillSummary, SystemPromptInfo };

/** Desktop I/O surface used by App — Tauri in production, fixture offline. */
export type DesktopBackend = {
  readonly mode: "tauri" | "fixture";
  listProviders(): Promise<ProviderRow[]>;
  listExternalAgents(): Promise<ExternalAgentRow[]>;
  setExternalAgent(id: string, enabled: boolean): Promise<void>;
  setExternalAgentMcp(id: string, enabled: boolean): Promise<void>;
  setExternalAgentModel(id: string, model: string | null): Promise<void>;
  checkExternalAgent(id: string): Promise<ExternalAgentCheck>;
  listMcpServers(): Promise<McpServerRow[]>;
  /** Saving also refreshes the server's tool list; every row comes back. */
  saveMcpServer(input: {
    id: string;
    command: string;
    args: string[];
    url?: string;
    headers?: Record<string, string>;
    headerSecrets?: Record<string, string>;
    envVars: string[];
    enabled: boolean;
    timeoutSecs?: number | null;
  }): Promise<McpServerRow[]>;
  setMcpServerEnabled(id: string, enabled: boolean): Promise<McpServerRow[]>;
  removeMcpServer(id: string): Promise<McpServerRow[]>;
  checkMcpServer(id: string): Promise<McpCheck>;
  setProviderKey(id: string, key: string): Promise<void>;
  deleteProviderKey(id: string): Promise<void>;
  providerKeyPresent(id: string): Promise<boolean>;
  configureApiProvider(input: {
    id: string;
    baseUrl: string;
    model: string;
    models: string[];
    credential: string;
    key: string;
  }): Promise<void>;
  configureAnthropicProvider(input: {
    id: string;
    model: string;
    credential: string;
    key: string;
  }): Promise<void>;
  configureClaudeCodeProvider(input: { id: string; model: string }): Promise<void>;
  configureCodexCliProvider(input: { id: string; model: string }): Promise<void>;
  configureCodexOAuthProvider(input: { id: string; model: string }): Promise<void>;
  codexCliAvailable(): Promise<boolean>;
  openProjectConfig(root: string): Promise<void>;
  usageSnapshot(): Promise<UsageSnapshot>;
  providerQuota(): Promise<ProviderQuotaSnapshot>;
  listPlugins(): Promise<PluginView[]>;
  openPluginsFolder(): Promise<void>;
  setPluginEnabled(id: string, enabled: boolean): Promise<PluginView[]>;
  nowPlaying(): Promise<NowPlayingView>;
  controlNowPlaying(action: "previous" | "toggle" | "next"): Promise<NowPlayingView>;
  setNowPlayingVolume(volumePercent: number): Promise<NowPlayingView>;
  wallpaper(): Promise<WallpaperView>;
  pickWallpaper(): Promise<WallpaperView>;
  setWallpaperFilter(filter: WallpaperFilterId): Promise<WallpaperView>;
  clearWallpaper(): Promise<WallpaperView>;
  usageReport(days: number): Promise<UsageReport>;
  /** Open the price book in the OS editor so rates can be corrected. */
  openPricesFile(): Promise<void>;
  /**
   * Fetch the published rate table if the cached copy is due.
   *
   * Separate from `usageReport` on purpose: the report must stay instant and
   * must not fail because the network is down. Call both, and re-read the
   * report only if the rates actually moved.
   */
  refreshRates(force: boolean): Promise<RatesStatus>;
  profileStats(): Promise<ProfileStats>;
  /** Hand core this machine's UTC offset so day boundaries match the clock. */
  setLocalOffset(): Promise<void>;
  lastProvider(): Promise<string | null>;
  startLogin(id: string): Promise<LoginStarted>;
  loginStatus(): Promise<LoginStatus>;
  cancelLogin(): Promise<void>;
  startSession(
    id: string,
    options?: { model?: string; effort?: string }
  ): Promise<SessionInfo>;
  updateSessionOptions(options: {
    model?: string;
    effort?: string;
  }): Promise<SessionMeta>;
  resetSessionOptions(): Promise<SessionMeta>;
  listThreads(): Promise<ThreadSummary[]>;
  forgetWorkspace(projectPath: string): Promise<void>;
  listChatProjects(): Promise<ProjectChats[]>;
  searchChats(query: string): Promise<ChatSearchHit[]>;
  openProjectChat(options: {
    /** `null` opens a chat without a workspace. */
    root: string | null;
    threadId?: string | null;
    newThread?: boolean;
    providerId?: string | null;
    copyThread?: boolean;
  }): Promise<SessionInfo>;
  loadThread(id: string): Promise<SessionInfo>;
  newThread(): Promise<SessionInfo>;
  sessionInfo(): Promise<SessionInfo | null>;
  forkThread(): Promise<SessionInfo>;
  forkThreadFromCheckpoint(checkpointId: string): Promise<SessionInfo>;
  rewindThread(checkpointId: string): Promise<SessionInfo>;
  editMessage(messageId: string): Promise<SessionInfo>;
  compactContext(): Promise<CompactionResult>;
  deleteThread(
    id: string,
    projectPath?: string | null,
    freeChat?: boolean
  ): Promise<SessionInfo>;
  setThreadPinned(
    id: string,
    projectPath: string | null | undefined,
    pinned: boolean,
    freeChat?: boolean
  ): Promise<void>;
  renameThread(
    id: string,
    projectPath: string | null | undefined,
    title: string,
    freeChat?: boolean
  ): Promise<ThreadSummary>;
  sendMessage(
    text: string,
    attachments?: AttachmentInput[],
    target?: InputTarget
  ): Promise<void>;
  updateQueuedInput(threadId: string, inputId: string, text: string): Promise<void>;
  removeQueuedInput(threadId: string, inputId: string): Promise<void>;
  resumeQueuedInputs(threadId: string): Promise<void>;
  listJobs(threadId?: string): Promise<JobSnapshot[]>;
  jobOutput(
    jobId: string,
    options?: {
      offset?: number;
      wait?: boolean;
      timeoutMs?: number;
      threadId?: string;
    }
  ): Promise<JobRead>;
  jobKill(jobId: string, reason?: string, threadId?: string): Promise<JobSnapshot>;
  saveMarkdown(suggestedName: string, markdown: string): Promise<string | null>;
  cancelTurn(threadId?: string): Promise<void>;
  resolveApproval(
    approvalId: string,
    decision: ApprovalChoice,
    threadId?: string
  ): Promise<void>;
  resolveQuestion(
    questionId: string,
    answer: string,
    threadId?: string
  ): Promise<void>;
  setApprovalMode(mode: ApprovalMode): Promise<string>;
  approvalMode(): Promise<string>;
  verifyProvider(id: string): Promise<void>;
  listCommands(): Promise<CommandView[]>;
  endSession(): Promise<void>;
  getSystemPrompt(): Promise<SystemPromptInfo>;
  setSystemPrompt(custom: string): Promise<SystemPromptInfo>;
  listSkills(): Promise<SkillSummary[]>;
  getWorkspaceFolder(): Promise<string>;
  /** Show the active project in the OS file manager. Rejects a projectless chat. */
  revealWorkspaceFolder(): Promise<void>;
  listWorkspaceFiles(relativePath?: string | null): Promise<WorkspaceFileView[]>;
  readWorkspaceFile(relativePath: string): Promise<WorkspaceFileContent>;
  pickWorkspaceFolder(): Promise<WorkspacePickResult | null>;
  pickFiles(): Promise<PreparedAttachment[]>;
  preparePastedImage(options: {
    dataBase64: string;
    mediaType: string;
    name?: string;
  }): Promise<PreparedAttachment>;
  gitBranch(): Promise<string | null>;
  gitContext(): Promise<GitContext>;
  workspaceChanges(): Promise<WorkspaceChange>;
  verifyWorkspace(): Promise<WorkspaceReview>;
  contextUsage(): Promise<ContextUsage>;
  getUserProfile(): Promise<UserProfile>;
  setUserProfile(profile: UserProfile): Promise<UserProfile>;
  onChatEvent(handler: (event: ChatEvent) => void): Promise<UnlistenFn>;
  listDelegationJobs(): Promise<DelegationJob[]>;
  listDelegationTargets(): Promise<DelegationTargetOptionView[]>;
  createDelegationJob(request: DelegationCreateInput): Promise<DelegationJob>;
  updateDelegationJob(request: DelegationUpdateInput): Promise<DelegationJob>;
  approveDelegationJob(jobId: string): Promise<DelegationJob>;
  prepareDelegationHandoff(jobId: string): Promise<DelegationHandoffView>;
  getDelegationJob(jobId: string): Promise<DelegationJob>;
  cancelDelegationJob(jobId: string): Promise<DelegationJob>;
  retryDelegationJob(jobId: string): Promise<DelegationJob>;
  applyDelegationJob(jobId: string): Promise<DelegationJob>;
  onDelegationEvent(handler: (event: DelegationEvent) => void): Promise<UnlistenFn>;
  /** Optional boot hook (fixture streams a canned turn). */
  boot?(handler: (event: ChatEvent) => void): Promise<void> | void;
};

export function createTauriBackend(): DesktopBackend {
  return {
    mode: "tauri",
    listProviders: () => tauriApi.listProviders(),
    listExternalAgents: () => tauriApi.listExternalAgents(),
    setExternalAgent: (id, enabled) => tauriApi.setExternalAgent(id, enabled),
    setExternalAgentMcp: (id, enabled) => tauriApi.setExternalAgentMcp(id, enabled),
    setExternalAgentModel: (id, model) => tauriApi.setExternalAgentModel(id, model),
    checkExternalAgent: (id) => tauriApi.checkExternalAgent(id),
    listMcpServers: () => tauriApi.listMcpServers(),
    saveMcpServer: (input) => tauriApi.saveMcpServer(input),
    setMcpServerEnabled: (id, enabled) => tauriApi.setMcpServerEnabled(id, enabled),
    removeMcpServer: (id) => tauriApi.removeMcpServer(id),
    checkMcpServer: (id) => tauriApi.checkMcpServer(id),
    setProviderKey: (id, key) => tauriApi.setProviderKey(id, key),
    deleteProviderKey: (id) => tauriApi.deleteProviderKey(id),
    providerKeyPresent: (id) => tauriApi.providerKeyPresent(id),
    configureApiProvider: (input) => tauriApi.configureApiProvider(input),
    configureAnthropicProvider: (input) => tauriApi.configureAnthropicProvider(input),
    configureClaudeCodeProvider: (input) => tauriApi.configureClaudeCodeProvider(input),
    configureCodexCliProvider: (input) => tauriApi.configureCodexCliProvider(input),
    configureCodexOAuthProvider: (input) => tauriApi.configureCodexOAuthProvider(input),
    codexCliAvailable: () => tauriApi.codexCliAvailable(),
    openProjectConfig: (root) => tauriApi.openProjectConfig(root),
    usageSnapshot: () => tauriApi.usageSnapshot(),
    providerQuota: () => tauriApi.providerQuota(),
    listPlugins: () => tauriApi.listPlugins(),
    openPluginsFolder: () => tauriApi.openPluginsFolder(),
    setPluginEnabled: (id, enabled) => tauriApi.setPluginEnabled(id, enabled),
    nowPlaying: () => tauriApi.nowPlaying(),
    controlNowPlaying: (action) => tauriApi.controlNowPlaying(action),
    setNowPlayingVolume: (volumePercent) => tauriApi.setNowPlayingVolume(volumePercent),
    wallpaper: () => tauriApi.wallpaper(),
    pickWallpaper: () => tauriApi.pickWallpaper(),
    setWallpaperFilter: (filter) => tauriApi.setWallpaperFilter(filter),
    clearWallpaper: () => tauriApi.clearWallpaper(),
    usageReport: (days) => tauriApi.usageReport(days),
    openPricesFile: () => tauriApi.openPricesFile(),
    refreshRates: (force) => tauriApi.refreshRates(force),
    profileStats: () => tauriApi.profileStats(),
    setLocalOffset: () => tauriApi.setLocalOffset(),
    lastProvider: () => tauriApi.lastProvider(),
    startLogin: (id) => tauriApi.startLogin(id),
    loginStatus: () => tauriApi.loginStatus(),
    cancelLogin: () => tauriApi.cancelLogin(),
    startSession: (id, options) => tauriApi.startSession(id, options),
    updateSessionOptions: (options) => tauriApi.updateSessionOptions(options),
    resetSessionOptions: () => tauriApi.resetSessionOptions(),
    listThreads: () => tauriApi.listThreads(),
    forgetWorkspace: (projectPath) => tauriApi.forgetWorkspace(projectPath),
    listChatProjects: () => tauriApi.listChatProjects(),
    searchChats: (query) => tauriApi.searchChats(query),
    openProjectChat: (options) => tauriApi.openProjectChat(options),
    loadThread: (id) => tauriApi.loadThread(id),
    newThread: () => tauriApi.newThread(),
    sessionInfo: () => tauriApi.sessionInfo(),
    forkThread: () => tauriApi.forkThread(),
    forkThreadFromCheckpoint: (checkpointId) => tauriApi.forkThreadFromCheckpoint(checkpointId),
    rewindThread: (checkpointId) => tauriApi.rewindThread(checkpointId),
    editMessage: (messageId) => tauriApi.editMessage(messageId),
    compactContext: () => tauriApi.compactContext(),
    deleteThread: (id, projectPath, freeChat) =>
      tauriApi.deleteThread(id, projectPath, freeChat),
    setThreadPinned: (id, projectPath, pinned, freeChat) =>
      tauriApi.setThreadPinned(id, projectPath, pinned, freeChat ?? projectPath == null),
    renameThread: (id, projectPath, title, freeChat) =>
      tauriApi.renameThread(id, projectPath, title, freeChat ?? projectPath == null),
    sendMessage: (text, attachments, target) =>
      tauriApi.sendMessage(text, attachments, target),
    updateQueuedInput: (threadId, inputId, text) =>
      tauriApi.updateQueuedInput(threadId, inputId, text),
    removeQueuedInput: (threadId, inputId) =>
      tauriApi.removeQueuedInput(threadId, inputId),
    resumeQueuedInputs: (threadId) => tauriApi.resumeQueuedInputs(threadId),
    listJobs: (threadId) => tauriApi.listJobs(threadId),
    jobOutput: (jobId, options) => tauriApi.jobOutput(jobId, options),
    jobKill: (jobId, reason, threadId) => tauriApi.jobKill(jobId, reason, threadId),
    saveMarkdown: (suggestedName, markdown) =>
      tauriApi.saveMarkdown(suggestedName, markdown),
    cancelTurn: (threadId) => tauriApi.cancelTurn(threadId),
    resolveApproval: (approvalId, decision, threadId) =>
      tauriApi.resolveApproval(approvalId, decision, threadId),
    resolveQuestion: (questionId, answer, threadId) =>
      tauriApi.resolveQuestion(questionId, answer, threadId),
    setApprovalMode: (mode) => tauriApi.setApprovalMode(mode),
    approvalMode: () => tauriApi.approvalMode(),
    verifyProvider: (id) => tauriApi.verifyProvider(id),
    listCommands: () => tauriApi.listCommands(),
    endSession: () => tauriApi.endSession(),
    getSystemPrompt: () => tauriApi.getSystemPrompt(),
    setSystemPrompt: (custom) => tauriApi.setSystemPrompt(custom),
    listSkills: () => tauriApi.listSkills(),
    getWorkspaceFolder: () => tauriApi.getWorkspaceFolder(),
    revealWorkspaceFolder: () => tauriApi.revealWorkspaceFolder(),
    listWorkspaceFiles: (relativePath) => tauriApi.listWorkspaceFiles(relativePath),
    readWorkspaceFile: (relativePath) => tauriApi.readWorkspaceFile(relativePath),
    pickWorkspaceFolder: () => tauriApi.pickWorkspaceFolder(),
    pickFiles: () => tauriApi.pickFiles(),
    preparePastedImage: (options) => tauriApi.preparePastedImage(options),
    gitBranch: () => tauriApi.gitBranch(),
    gitContext: () => tauriApi.gitContext(),
    workspaceChanges: () => tauriApi.workspaceChanges(),
    verifyWorkspace: () => tauriApi.verifyWorkspace(),
    contextUsage: () => tauriApi.contextUsage(),
    getUserProfile: () => tauriApi.getUserProfile(),
    setUserProfile: (profile) => tauriApi.setUserProfile(profile),
    onChatEvent: (handler) => tauriApi.onChatEvent(handler),
    listDelegationJobs: () => tauriApi.listDelegationJobs(),
    listDelegationTargets: () => tauriApi.listDelegationTargets(),
    createDelegationJob: (request) => tauriApi.createDelegationJob(request),
    updateDelegationJob: (request) => tauriApi.updateDelegationJob(request),
    approveDelegationJob: (jobId) => tauriApi.approveDelegationJob(jobId),
    prepareDelegationHandoff: (jobId) => tauriApi.prepareDelegationHandoff(jobId),
    getDelegationJob: (jobId) => tauriApi.getDelegationJob(jobId),
    cancelDelegationJob: (jobId) => tauriApi.cancelDelegationJob(jobId),
    retryDelegationJob: (jobId) => tauriApi.retryDelegationJob(jobId),
    applyDelegationJob: (jobId) => tauriApi.applyDelegationJob(jobId),
    onDelegationEvent: (handler) => tauriApi.onDelegationEvent(handler),
  };
}

export function selectBackend(): DesktopBackend {
  //  is replaced with a literal at build time, so the
  // fixture import below is statically unreachable in a release and the whole
  // module is dropped. It also means  is a dev-server affordance
  // rather than something a shipped app will answer to.
  const fixture =
    import.meta.env.DEV &&
    typeof window !== "undefined" &&
    new URLSearchParams(window.location.search).has("fixture");
  return fixture ? createFixtureBackend() : createTauriBackend();
}

let sharedBackend: DesktopBackend | null = null;

/** Process-wide backend (fixture keeps in-memory session state). */
export function getBackend(): DesktopBackend {
  if (!sharedBackend) sharedBackend = selectBackend();
  return sharedBackend;
}
