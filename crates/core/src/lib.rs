//! Zest core: provider layer, agent loop, tool layer.
//!
//! Deliberately headless — no UI, no terminal assumptions. The CLI crate is one
//! front-end; a desktop app would be another.

pub mod agent;
#[cfg(test)]
mod alpha_prove;
pub mod anthropic;
pub mod auth;
pub mod bounded;
pub mod cancel;
pub mod chat_persistence;
pub mod codex_oauth;
pub mod commands;
pub mod config;
pub mod config_edit;
pub mod config_migrate;
pub mod context_budget;
pub mod credentials;
pub mod delegated_worker;
pub mod delegation;
pub mod error;
pub mod fsutil;
pub mod handoff;
pub mod inbox;
pub mod jobs;
pub mod mcp;
pub mod orchestration;
pub mod persist;
pub mod prefs;
pub mod pricing;
pub mod process_job;
pub mod profile;
pub mod prompt;
pub mod provider;
pub mod prune;
pub mod quota;
pub mod rates;
pub mod reading_diff;
pub mod runtime;
pub mod skills;
pub mod thread;
pub mod tools;
pub mod transcripts;
pub mod usage;
pub mod workspace_changes;

pub use agent::{Agent, CompactionOutcome, TurnUsageSummary};
pub use anthropic::client::AnthropicClient;
pub use anthropic::types::{
    tool_result, tool_uses, Message, OutputConfig, Request, Thinking, ToolDef, ToolUse, Usage,
    DEFAULT_MODEL,
};
pub use auth::{
    can_start_login, codex_cli_on_path, detect_all, detect_claude_code, detect_codex_cli,
    detect_codex_oauth, resolve_claude_code_login, resolve_codex_cli_login, resolve_login,
    start_claude_code_login, start_codex_cli_login, start_codex_oauth_login, start_login,
    AuthStatus, LoginPoll, LoginProcess, LoginSpawn, ProviderSlot,
};
pub use cancel::{wait_cancel, CancelToken};
pub use chat_persistence::{
    ChatPersistence, InterruptRecord, InterruptStatus, InterruptStore, ReconstructedChat,
    RecoverableRun, RecoveryReconciliation, RunError, RunPatch, RunRecord, RunStatus, RunStore,
    RunUsage,
};
pub use commands::{
    expand as expand_command, expand_as as expand_command_as, list_slash_commands, mcp_slashes,
    parse_command, Expansion, McpSlash, ParsedCommand, SlashCommand, SlashKind,
};
pub use config::{
    ensure_user_config, load_env, user_config_path, ClaudeCodePermissionMode, Config,
    ExternalAgentConfig, ExternalAgentMode, ExternalWorkspace, McpServerConfig, ProviderConfig,
    Target, DEFAULT_CLAUDE_CODE_MODEL, DEFAULT_CODEX_MODEL, DEFAULT_USER_CONFIG,
};
pub use delegated_worker::{
    run_acceptance_checks, run_provider_reviewer, run_provider_worker, NativeReviewerResult,
    NativeWorkerResult, ResolvedWorkerMetadata, MAX_NATIVE_RESULT_CHARS,
};
pub use delegation::{
    apply_diff_checked, capture_workspace_snapshot, capture_worktree_lineage, dependency_blocker,
    diff_paths, validate_diff_paths, validate_diff_scope, validate_review_paths,
    AcceptanceCheckResult, AttemptRole, AttemptUsage, CheckStatus, DelegationArtifacts,
    DelegationAttempt, DelegationJob, DelegationOrigin, DelegationStatus, DelegationStore,
    DelegationTarget, FeatureCard, ResolvedTargetMetadata, ReviewDecision, ReviewFinding,
    ReviewReport, ReviewSeverity, ReviewerTarget, TargetFingerprint, WorkerResult,
    WorkspaceSnapshot, DELEGATION_FORMAT_VERSION, LEGACY_DELEGATION_FORMAT_VERSION,
};
pub use error::{HarnessError, Result};
pub use fsutil::{atomic_write, atomic_write_json, display_path, display_path_str};
pub use handoff::{ContextHandoff, MAX_HANDOFF_BYTES};
pub use inbox::InputInbox;
pub use jobs::{JobEvent, JobOutput, JobRead, JobRegistry, JobSnapshot, JobStatus};
pub use mcp::{
    probe_server as probe_mcp_server, register_mcp_tools, McpCatalog, McpServer, McpToolDef,
};
pub use orchestration::{
    DecisionGate, DispatchRole, DispatchState, DispatchStatus, ExternalSessionEvidence, GateStatus,
    InboxMessage, LifecycleEntry, LifecyclePhase, MessageKind, OrchestrationState, RetryState,
    WorktreeLineage, ORCHESTRATION_FORMAT_VERSION,
};
pub use persist::{
    PersistPriority, PersistWorker, Snapshot as PersistSnapshot, DELTA_CHECKPOINT_MS,
};
pub use prefs::{ProjectSessionState, ProviderSessionPrefs};
pub use pricing::{ModelPrice, Prices};
pub use profile::{derive as derive_profile_stats, ChatFacts, DayPoint, ProfileStats};
pub use prompt::{
    compose_for_project, compose_system, compose_system_with_docs, custom_system_path, env_context,
    load_custom_system, load_project_docs, save_custom_system, truncate_chars, DEFAULT_SYSTEM,
    EXTERNAL_DELEGATION_SYSTEM, LOCAL_BROWSER_SYSTEM, MAX_CUSTOM_PROMPT_BYTES,
    MAX_PROJECT_DOCS_BYTES, PROJECT_DOC_FILES,
};
pub use provider::anthropic::AnthropicProvider;
pub use provider::claude_code::ClaudeCodeProvider;
pub use provider::codex_app_server::CodexAppServerProvider;
pub use provider::codex_oauth::CodexOAuthProvider;
pub use provider::driver::{
    credentials_for, driver_for, CredentialPolicy, CredentialRequest, DriverContext, DriverKind,
    ProviderDriver, BUILT_IN_DRIVERS,
};
pub use provider::registry::{ProviderRegistry, Skipped};
pub use provider::{
    catalogue, context_window_for_model, descriptor_for_picker_id, descriptor_from_config,
    normalize_effort, probe, thread_provider_handoff, Completion, EffortPolicy, ModelSpec,
    Provider, ProviderCommandRequest, ProviderDescriptor, ProviderFileChangeRequest,
    ProviderInteractionHost, ProviderQuestionRequest, ProviderSessionRef, RateLimitSnapshot,
    ResumeHandle, ResumeSupport, StreamEvent, SystemPrompt, ThreadProviderHandoff, TurnRequest,
    CODEX_KNOWN_MODELS, STANDARD_EFFORTS,
};
pub use quota::{
    fetch_provider_quotas, ProviderBalanceView, ProviderQuotaKind, ProviderQuotaSnapshot,
    ProviderQuotaView, ProviderQuotaWindowView, ProviderSpendLimitView,
};
pub use rates::{RateCatalog, DEFAULT_RATES_URL};
pub use reading_diff::{
    abridge as abridge_reading_diff, LineRange, ReadingDiffPlan, ReadingDiffResult,
};
pub use runtime::{
    resolve_provider_target, ResolvedProviderTarget, RuntimeBuilder, RuntimeRole, RuntimeSession,
};
pub use skills::{
    Skill, SkillSet, SkillSummary, INLINE_BUDGET_BYTES, INLINE_MAX_BYTES, MAX_SKILLS,
    MAX_SKILL_BYTES,
};
pub use thread::{
    contains_ignore_ascii_case, match_excerpt, new_id, thread_summary_from_json, PullRequestLink,
    StoredMessage, Thread, ThreadCheckpoint, ThreadCheckpointKind, ThreadEvent, ThreadEventKind,
    ThreadGitContext, ThreadId, ThreadInput, ThreadInputAttachment, ThreadInputTarget, ThreadLoad,
    ThreadLoadError, ThreadStore, ThreadSummary, ToolPart as ThreadToolPart, THREAD_FORMAT_VERSION,
    WIRE_FORMAT_ANTHROPIC_MESSAGES,
};
pub use tools::approval::{
    AllowApprover, ApprovalDecision, ApprovalMode, ApprovalPolicy, ApprovalPreview,
    ApprovalRequest, Approver, DenyApprover, PolicyOutcome, ToolRisk,
};
pub use tools::browser::{BrowserAction, BrowserAdapter, BrowserLocator, BrowserRequest};
pub use tools::external_agent::{
    prepare_external_command, resolve_program, run_delegation_reviewer, run_delegation_worker,
    ExternalAgent, ExternalAgentResult, EXTERNAL_AGENT_TOOL,
};
pub use tools::glob_files::GlobFiles;
pub use tools::grep::Grep;
pub use tools::list_dir::ListDir;
pub use tools::prepared::{PreImage, PreparedToolCall};
pub use tools::question::{
    parse_question_input, AskUser, DenyQuestioner, QuestionRequest, Questioner, ASK_USER_TOOL,
};
pub use tools::read_file::ReadFile;
pub use tools::sensitive::is_sensitive_path;
pub use tools::write_file::WriteFile;
pub use tools::{
    register_browser_tool, register_exec_tools, register_exec_tools_with_jobs, register_job_tools,
    register_question_tool, register_read_tools, register_skill_tools, register_write_tools,
    FeatureDelegator, Tool, ToolMetadata, ToolOutcome, ToolRegistry, DELEGATE_FEATURE_TOOL,
};
pub use transcripts::{CliKind, ScanResult, ScanStatus};
pub use usage::{
    CostQuality, CostSource, DayCostPoint, ExternalCost, ExternalUsageReport,
    ExternalWorkerUsageView, HeadroomView, Ledger, MeasuredUsage, ModelCostRow, PromptCacheShares,
    ProviderCostRow, ProviderDayPoint, ProviderUsage, ProviderUsageView, RangeTotals, RatesStatus,
    TokenCounts, UsageReport, UsageSnapshot, DAILY_RETENTION_DAYS,
};
pub use workspace_changes::{FileChangeSummary, WorkspaceChangeSet};
