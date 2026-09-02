import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

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
  GitContext,
  InputTarget,
  JobRead,
  JobSnapshot,
  ExternalAgentCheck,
  ExternalAgentRow,
  LoginStarted,
  LoginStatus,
  McpCheck,
  McpServerRow,
  NowPlayingView,
  PreparedAttachment,
  PluginView,
  ProfileStats,
  ProviderRow,
  ProviderQuotaSnapshot,
  RatesStatus,
  OlderThreadMessages,
  SessionInfo,
  SessionMeta,
  ProjectChats,
  ChatSearchHit,
  ThreadSummary,
  UsageReport,
  UsageSnapshot,
  UserProfile,
  WallpaperFilterId,
  WallpaperView,
  WorkspaceFileContent,
  WorkspaceFileView,
  WorkspacePickResult,
  WorkspaceChange,
  WorkspaceReview,
} from "./types";

export function listProviders() {
  return invoke<ProviderRow[]>("list_providers");
}

export function listExternalAgents() {
  return invoke<ExternalAgentRow[]>("list_external_agents");
}

export function setExternalAgent(id: string, enabled: boolean) {
  return invoke<void>("set_external_agent", { id, enabled });
}

export function setExternalAgentMcp(id: string, enabled: boolean) {
  return invoke<void>("set_external_agent_mcp", { id, enabled });
}

export function setExternalAgentModel(id: string, model: string | null) {
  return invoke<void>("set_external_agent_model", { id, model });
}

export function checkExternalAgent(id: string) {
  return invoke<ExternalAgentCheck>("check_external_agent", { id });
}

export function listMcpServers() {
  return invoke<McpServerRow[]>("list_mcp_servers");
}

/** Writes the whole entry, then refreshes its tool list. Returns every row. */
export function saveMcpServer(input: {
  id: string;
  command: string;
  args: string[];
  url?: string;
  headers?: Record<string, string>;
  headerSecrets?: Record<string, string>;
  envVars: string[];
  enabled: boolean;
  timeoutSecs?: number | null;
}) {
  return invoke<McpServerRow[]>("save_mcp_server", { input });
}

export function setMcpServerEnabled(id: string, enabled: boolean) {
  return invoke<McpServerRow[]>("set_mcp_server_enabled", { id, enabled });
}

export function removeMcpServer(id: string) {
  return invoke<McpServerRow[]>("remove_mcp_server", { id });
}

export function checkMcpServer(id: string) {
  return invoke<McpCheck>("check_mcp_server", { id });
}

export function setProviderKey(id: string, key: string) {
  return invoke<void>("set_provider_key", { id, key });
}

export function deleteProviderKey(id: string) {
  return invoke<void>("delete_provider_key", { id });
}

export function providerKeyPresent(id: string) {
  return invoke<boolean>("provider_key_present", { id });
}

export function configureApiProvider(input: {
  id: string;
  baseUrl: string;
  model: string;
  models: string[];
  credential: string;
  key: string;
}) {
  return invoke<void>("configure_api_provider", input);
}

export function configureAnthropicProvider(input: {
    id: string;
    model: string;
    credential: string;
    key: string;
}) {
  return invoke<void>("configure_anthropic_provider", input);
}

export function configureClaudeCodeProvider(input: { id: string; model: string }) {
  return invoke<void>("configure_claude_code_provider", input);
}

export function configureCodexCliProvider(input: { id: string; model: string }) {
  return invoke<void>("configure_codex_cli_provider", input);
}

export function configureCodexOAuthProvider(input: { id: string; model: string }) {
  return invoke<void>("configure_codex_oauth_provider", input);
}

export function codexCliAvailable() {
  return invoke<boolean>("codex_cli_available");
}

export function openProjectConfig(root: string) {
  return invoke<void>("open_project_config", { root });
}

export function usageSnapshot() {
  return invoke<UsageSnapshot>("usage_snapshot");
}

export function providerQuota() {
  return invoke<ProviderQuotaSnapshot>("provider_quota");
}

export function listPlugins() {
  return invoke<PluginView[]>("list_plugins");
}

export function openPluginsFolder() {
  return invoke<void>("open_plugins_folder");
}

export function setPluginEnabled(id: string, enabled: boolean) {
  return invoke<PluginView[]>("set_plugin_enabled", { id, enabled });
}

export function nowPlaying() {
  return invoke<NowPlayingView>("now_playing");
}

export function controlNowPlaying(action: "previous" | "toggle" | "next") {
  return invoke<NowPlayingView>("control_now_playing", { action });
}

export function setNowPlayingVolume(volumePercent: number) {
  return invoke<NowPlayingView>("set_now_playing_volume", { volumePercent });
}

export function wallpaper() {
  return invoke<WallpaperView>("wallpaper");
}

export function pickWallpaper() {
  return invoke<WallpaperView>("pick_wallpaper");
}

export function setWallpaperFilter(filter: WallpaperFilterId) {
  return invoke<WallpaperView>("set_wallpaper_filter", { filter });
}

export function clearWallpaper() {
  return invoke<WallpaperView>("clear_wallpaper");
}

export function usageReport(days: number) {
  return invoke<UsageReport>("usage_report", { days });
}

export function openPricesFile() {
  return invoke<void>("open_prices_file");
}

export function refreshRates(force: boolean) {
  return invoke<RatesStatus>("refresh_rates", { force });
}

export function profileStats() {
  return invoke<ProfileStats>("profile_stats");
}

/**
 * Hand core this machine's UTC offset.
 *
 * The webview is the only part of Zest that knows the timezone, and every day
 * boundary depends on it. `getTimezoneOffset` reports minutes *behind* UTC, so
 * the sign is flipped to the usual "minutes east" convention.
 */
export function setLocalOffset(minutes = -new Date().getTimezoneOffset()) {
  return invoke<void>("set_local_offset", { minutes });
}

export function lastProvider() {
  return invoke<string | null>("last_provider");
}

export function startLogin(id: string) {
  return invoke<LoginStarted>("start_login", { id });
}

export function loginStatus() {
  return invoke<LoginStatus>("login_status");
}

export function cancelLogin() {
  return invoke<void>("cancel_login");
}

export function startSession(
  id: string,
  options?: { model?: string; effort?: string }
) {
  return invoke<SessionInfo>("start_session", {
    id,
    model: options?.model ?? null,
    effort: options?.effort ?? null,
  });
}

export function switchSessionProvider(providerId: string, model?: string) {
  return invoke<SessionInfo>("switch_session_provider", {
    providerId,
    model: model ?? null,
  });
}

export function updateSessionOptions(options: {
  model?: string;
  effort?: string;
}) {
  return invoke<SessionMeta>("update_session_options", {
    model: options.model ?? null,
    effort: options.effort ?? null,
  });
}

export function resetSessionOptions() {
  return invoke<SessionMeta>("reset_session_options");
}

export function listThreads() {
  return invoke<ThreadSummary[]>("list_threads");
}

export function forgetWorkspace(projectPath: string) {
  return invoke<void>("forget_workspace", { projectPath });
}

export function listChatProjects() {
  return invoke<ProjectChats[]>("list_chat_projects");
}

export function searchChats(query: string) {
  return invoke<ChatSearchHit[]>("search_chats", { query });
}

export function openProjectChat(options: {
  root: string | null;
  threadId?: string | null;
  newThread?: boolean;
  providerId?: string | null;
  copyThread?: boolean;
  focusMessageId?: string | null;
}) {
  return invoke<SessionInfo>("open_project_chat", {
    root: options.root,
    threadId: options.threadId ?? null,
    newThread: options.newThread ?? null,
    providerId: options.providerId ?? null,
    copyThread: options.copyThread ?? null,
    focusMessageId: options.focusMessageId ?? null,
  });
}

export function loadOlderThreadMessages(options: {
  threadId: string;
  beforeMessageId: string;
}) {
  return invoke<OlderThreadMessages>("load_older_thread_messages", {
    threadId: options.threadId,
    beforeMessageId: options.beforeMessageId,
  });
}

export function loadNewerThreadMessages(options: {
  threadId: string;
  afterMessageId: string;
}) {
  return invoke<OlderThreadMessages>("load_newer_thread_messages", {
    threadId: options.threadId,
    afterMessageId: options.afterMessageId,
  });
}

export function loadThread(id: string) {
  return invoke<SessionInfo>("load_thread", { id });
}

export function newThread() {
  return invoke<SessionInfo>("new_thread");
}

export function sessionInfo() {
  return invoke<SessionInfo | null>("session_info");
}

export function forkThread() {
  return invoke<SessionInfo>("fork_thread");
}

export function forkThreadFromCheckpoint(checkpointId: string) {
  return invoke<SessionInfo>("fork_thread_from_checkpoint", { checkpointId });
}

export function rewindThread(checkpointId: string) {
  return invoke<SessionInfo>("rewind_thread", { checkpointId });
}

export function editMessage(messageId: string) {
  return invoke<SessionInfo>("edit_message", { messageId });
}

export function compactContext() {
  return invoke<CompactionResult>("compact_context");
}

export function deleteThread(
  id: string,
  projectPath?: string | null,
  freeChat = false
) {
  return invoke<SessionInfo>("delete_thread", {
    id,
    projectPath: projectPath ?? null,
    freeChat,
  });
}

export function setThreadPinned(
  id: string,
  projectPath: string | null | undefined,
  pinned: boolean,
  freeChat = false
) {
  return invoke<void>("set_thread_pinned", {
    id,
    projectPath: projectPath ?? null,
    freeChat,
    pinned,
  });
}

export function renameThread(
  id: string,
  projectPath: string | null | undefined,
  title: string,
  freeChat = false
) {
  return invoke<ThreadSummary>("rename_thread", {
    id,
    projectPath: projectPath ?? null,
    freeChat,
    title,
  });
}

export function sendMessage(
  text: string,
  attachments?: AttachmentInput[],
  target?: InputTarget,
) {
  return invoke<void>("send_message", {
    text,
    attachments: attachments ?? null,
    target: target ?? null,
  });
}

export function updateQueuedInput(threadId: string, inputId: string, text: string) {
  return invoke<void>("update_queued_input", { threadId, inputId, text });
}

export function removeQueuedInput(threadId: string, inputId: string) {
  return invoke<void>("remove_queued_input", { threadId, inputId });
}

export function resumeQueuedInputs(threadId: string) {
  return invoke<void>("resume_queued_inputs", { threadId });
}

export function listJobs(threadId?: string) {
  return invoke<JobSnapshot[]>("list_jobs", { threadId: threadId ?? null });
}

export function jobOutput(
  jobId: string,
  options?: {
    offset?: number;
    wait?: boolean;
    timeoutMs?: number;
    threadId?: string;
  },
) {
  return invoke<JobRead>("job_output", {
    jobId,
    offset: options?.offset ?? null,
    wait: options?.wait ?? false,
    timeoutMs: options?.timeoutMs ?? null,
    threadId: options?.threadId ?? null,
  });
}

export function jobKill(jobId: string, reason?: string, threadId?: string) {
  return invoke<JobSnapshot>("job_kill", {
    jobId,
    reason: reason ?? null,
    threadId: threadId ?? null,
  });
}

export function saveMarkdown(suggestedName: string, markdown: string) {
  return invoke<string | null>("save_markdown", {
    suggestedName,
    markdown,
  });
}

export function revealWorkspaceFolder() {
  return invoke<void>("reveal_workspace_folder");
}

export function getWorkspaceFolder() {
  return invoke<string>("get_workspace_folder");
}

export function listWorkspaceFiles(relativePath?: string | null) {
  return invoke<WorkspaceFileView[]>("list_workspace_files", {
    relativePath: relativePath ?? null,
  });
}

export function readWorkspaceFile(relativePath: string) {
  return invoke<WorkspaceFileContent>("read_workspace_file", { relativePath });
}

export function pickWorkspaceFolder() {
  return invoke<WorkspacePickResult | null>("pick_workspace_folder");
}

export function pickFiles() {
  return invoke<PreparedAttachment[]>("pick_files");
}

export function preparePastedImage(options: {
  dataBase64: string;
  mediaType: string;
  name?: string;
}) {
  return invoke<PreparedAttachment>("prepare_pasted_image", {
    dataBase64: options.dataBase64,
    mediaType: options.mediaType,
    name: options.name ?? null,
  });
}

export function gitBranch() {
  return invoke<string | null>("git_branch");
}

export function gitContext() {
  return invoke<GitContext>("git_context");
}

export function workspaceChanges() {
  return invoke<WorkspaceChange>("workspace_changes");
}

export function pullRequestDiff(number?: number) {
  return invoke<WorkspaceChange>("pull_request_diff", {
    number: number ?? null,
  });
}

export function verifyWorkspace() {
  return invoke<WorkspaceReview>("verify_workspace");
}

export function contextUsage() {
  return invoke<ContextUsage>("context_usage");
}

export function getUserProfile() {
  return invoke<UserProfile>("get_user_profile");
}

export function setUserProfile(profile: UserProfile) {
  return invoke<UserProfile>("set_user_profile", { profile });
}

export function cancelTurn(threadId?: string) {
  return invoke<void>("cancel_turn", { threadId: threadId ?? null });
}

export function resolveApproval(
  approvalId: string,
  decision: ApprovalChoice,
  threadId?: string
) {
  return invoke<void>("resolve_approval", {
    approvalId,
    decision,
    threadId: threadId ?? null,
  });
}

export function resolveQuestion(
  questionId: string,
  answer: string,
  threadId?: string
) {
  return invoke<void>("resolve_question", {
    questionId,
    answer,
    threadId: threadId ?? null,
  });
}

export type ReadingDiffView = {
  diff: string;
  summary: string;
  removedLines: number;
  foldedLines: number;
};

export function generateReadingDiff(diff: string) {
  return invoke<ReadingDiffView>("generate_reading_diff", { diff });
}

export function setApprovalMode(mode: ApprovalMode) {
  return invoke<string>("set_approval_mode", { mode });
}

export function verifyProvider(id: string) {
  return invoke<void>("verify_provider", { id });
}

export function listCommands() {
  return invoke<CommandView[]>("list_commands");
}

export function approvalMode() {
  return invoke<string>("approval_mode");
}

export function endSession() {
  return invoke<void>("end_session");
}

export function onChatEvent(handler: (event: ChatEvent) => void): Promise<UnlistenFn> {
  return listen<ChatEvent>("chat-event", (event) => handler(event.payload));
}

export function listDelegationJobs() {
  return invoke<DelegationJob[]>("list_delegation_jobs");
}

export function listDelegationTargets() {
  return invoke<DelegationTargetOptionView[]>("list_delegation_targets");
}

export function createDelegationJob(request: DelegationCreateInput) {
  return invoke<DelegationJob>("create_delegation_job", { request });
}

export function updateDelegationJob(request: DelegationUpdateInput) {
  return invoke<DelegationJob>("update_delegation_job", { request });
}

export function approveDelegationJob(jobId: string) {
  return invoke<DelegationJob>("approve_delegation_job", { jobId });
}

export function prepareDelegationHandoff(jobId: string) {
  return invoke<DelegationHandoffView>("prepare_delegation_handoff", { jobId });
}

export function getDelegationJob(jobId: string) {
  return invoke<DelegationJob>("get_delegation_job", { jobId });
}

export function cancelDelegationJob(jobId: string) {
  return invoke<DelegationJob>("cancel_delegation_job", { jobId });
}

export function retryDelegationJob(jobId: string) {
  return invoke<DelegationJob>("retry_delegation_job", { jobId });
}

export function applyDelegationJob(jobId: string) {
  return invoke<DelegationJob>("apply_delegation_job", { jobId });
}

export function onDelegationEvent(
  handler: (event: DelegationEvent) => void
): Promise<UnlistenFn> {
  return listen<DelegationEvent>("delegation-event", (event) => handler(event.payload));
}

export type SystemPromptInfo = {
  base: string;
  custom: string;
  composedPreview: string;
  customPath: string;
};

export type SkillSummary = {
  name: string;
  description: string;
  path: string;
  inlined: boolean;
};

export function getSystemPrompt() {
  return invoke<SystemPromptInfo>("get_system_prompt");
}

export function setSystemPrompt(custom: string) {
  return invoke<SystemPromptInfo>("set_system_prompt", { custom });
}

export function listSkills() {
  return invoke<SkillSummary[]>("list_skills");
}
