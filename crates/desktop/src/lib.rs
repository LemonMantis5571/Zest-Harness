//! Desktop front-end: provider picker + chat session.
//!
//! Codex Connect is a native shell over vendor OAuth (no token exchange in Zest).
//! Chat drives the same `Agent` loop as the CLI, streaming events into the UI.
//! Thread projection is persisted under `<workspace>/.zest/threads/`.

mod attachments;
mod browser;
mod context_meter;
mod delegation;
mod plugins;
mod session;
mod spaces;
mod turn;
mod workspace_files;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::process::Command;
use tokio::sync::oneshot;
#[cfg(feature = "export-bindings")]
use ts_rs::TS;
use zest_core::{
    can_start_login, compose_system_with_docs, derive_profile_stats, descriptor_for_picker_id,
    descriptor_from_config, detect_all, detect_claude_code, detect_codex_cli, display_path,
    ensure_gateway_running, env_context, load_custom_system, load_project_docs, new_id, probe,
    save_custom_system, start_claude_code_login as core_start_claude_code_login,
    start_codex_cli_login as core_start_codex_cli_login, start_login as core_start_login,
    truncate_chars, ApprovalDecision, ApprovalMode, ApprovalPolicy, ApprovalRequest, Approver,
    AuthStatus, ChatFacts, ChatPersistence, CompactionOutcome, Config, ExternalAgentMode,
    ExternalWorkspace, GatewayLease, GatewayState, HarnessError, Ledger, LoginProcess,
    PersistPriority, PersistSnapshot, PersistWorker, Prices, ProfileStats, ProjectSessionState,
    ProviderCommandRequest, ProviderConfig, ProviderFileChangeRequest, ProviderInteractionHost,
    ProviderQuestionRequest, ProviderQuotaSnapshot, ProviderRegistry, ProviderSlot,
    PullRequestLink, QuestionRequest, Questioner, RatesStatus, RecoverableRun, RuntimeBuilder,
    SkillSet, SkillSummary, StoredMessage, StreamEvent, SystemPrompt, Thread, ThreadCheckpoint,
    ThreadCheckpointKind, ThreadGitContext, ThreadLoadError, ThreadStore, ThreadSummary,
    ToolMetadata, ToolRisk, UsageReport, UsageSnapshot, DAILY_RETENTION_DAYS, DEFAULT_SYSTEM,
    THREAD_FORMAT_VERSION,
};

use attachments::{
    build_user_content, format_display_message, has_images, has_usable_attachment,
    prepare_image_bytes, prepare_paths, AttachmentInput, PreparedAttachment,
};
use browser::BrowserHost;
use context_meter::{estimate_context, CompactionResultView, ContextUsageView};
pub(crate) use delegation::{
    get_view as get_delegation_view, list_views as list_delegation_views, DelegationCoordinator,
    DelegationJobView,
};
use plugins::{NowPlayingView, PluginView};
use session::{Session, SessionController, SessionError};
use spaces::{SpaceState, DEFAULT_SPACE_ID};
use workspace_files::{WorkspaceFileContent, WorkspaceFileView};

/// Providers shown in the desktop launch picker.
///
/// Claude Code is available as a first-class parent and remains separately
/// configurable as a delegated worker. Gemini remains worker-only.
const PICKER_IDS: &[&str] = &["codex", "claude"];

/// Sign-in flows Zest can launch from the desktop. Claude here means the
/// first-class parent provider; worker authentication remains CLI-owned.
const DESKTOP_CONNECT_IDS: &[&str] = &["codex", "claude"];

fn desktop_can_start_login(provider_id: &str) -> bool {
    DESKTOP_CONNECT_IDS.contains(&provider_id) && can_start_login(provider_id)
}

/// Turn-scoped pending approval waiters (not persisted).
struct ApprovalHub {
    /// Active turn that may own waiters. Resolves outside this turn are rejected.
    active_turn: Mutex<Option<String>>,
    senders: Mutex<HashMap<String, oneshot::Sender<ApprovalDecision>>>,
    receivers: Mutex<HashMap<String, oneshot::Receiver<ApprovalDecision>>>,
}

impl ApprovalHub {
    fn new() -> Self {
        Self {
            active_turn: Mutex::new(None),
            senders: Mutex::new(HashMap::new()),
            receivers: Mutex::new(HashMap::new()),
        }
    }

    fn begin_turn(&self, turn_id: &str) {
        if let Ok(mut g) = self.active_turn.lock() {
            *g = Some(turn_id.to_string());
        }
    }

    fn prepare(&self, approval_id: &str) {
        let (tx, rx) = oneshot::channel();
        if let Ok(mut senders) = self.senders.lock() {
            senders.insert(approval_id.to_string(), tx);
        }
        if let Ok(mut receivers) = self.receivers.lock() {
            receivers.insert(approval_id.to_string(), rx);
        }
    }

    /// Anything that is not an explicit allow — a dropped sender, a poisoned
    /// lock, an unknown id — resolves to Deny.
    async fn wait(&self, approval_id: &str) -> ApprovalDecision {
        let rx = {
            let mut receivers = match self.receivers.lock() {
                Ok(g) => g,
                Err(_) => return ApprovalDecision::Deny,
            };
            receivers.remove(approval_id)
        };
        match rx {
            Some(rx) => rx.await.unwrap_or(ApprovalDecision::Deny),
            None => ApprovalDecision::Deny,
        }
    }

    fn resolve(&self, approval_id: &str, decision: ApprovalDecision) -> Result<(), String> {
        let turn_alive = self
            .active_turn
            .lock()
            .map_err(|_| "approval lock poisoned".to_string())?
            .is_some();
        if !turn_alive {
            return Err("no active turn for approval".into());
        }
        let mut senders = self
            .senders
            .lock()
            .map_err(|_| "approval lock poisoned".to_string())?;
        let tx = senders
            .remove(approval_id)
            .ok_or_else(|| "no pending approval with that id".to_string())?;
        let _ = tx.send(decision);
        Ok(())
    }

    /// Deny every waiter. Call after cancelling the turn token.
    fn clear(&self) {
        if let Ok(mut senders) = self.senders.lock() {
            for (_, tx) in senders.drain() {
                let _ = tx.send(ApprovalDecision::Deny);
            }
        }
        if let Ok(mut receivers) = self.receivers.lock() {
            receivers.clear();
        }
        if let Ok(mut g) = self.active_turn.lock() {
            *g = None;
        }
    }
}

struct HubApprover {
    hub: Arc<ApprovalHub>,
}

#[async_trait]
impl Approver for HubApprover {
    async fn prepare(&self, approval_id: &str) {
        self.hub.prepare(approval_id);
    }

    async fn decide(&self, request: &ApprovalRequest) -> ApprovalDecision {
        self.hub.wait(&request.approval_id).await
    }
}

/// Turn-scoped pending interactive questions (not persisted).
struct QuestionHub {
    active_turn: Mutex<Option<String>>,
    senders: Mutex<HashMap<String, oneshot::Sender<Result<String, String>>>>,
    receivers: Mutex<HashMap<String, oneshot::Receiver<Result<String, String>>>>,
}

impl QuestionHub {
    fn new() -> Self {
        Self {
            active_turn: Mutex::new(None),
            senders: Mutex::new(HashMap::new()),
            receivers: Mutex::new(HashMap::new()),
        }
    }

    fn begin_turn(&self, turn_id: &str) {
        if let Ok(mut guard) = self.active_turn.lock() {
            *guard = Some(turn_id.to_string());
        }
    }

    fn prepare(&self, question_id: &str) {
        let (tx, rx) = oneshot::channel();
        if let Ok(mut senders) = self.senders.lock() {
            senders.insert(question_id.to_string(), tx);
        }
        if let Ok(mut receivers) = self.receivers.lock() {
            receivers.insert(question_id.to_string(), rx);
        }
    }

    async fn wait(&self, question_id: &str) -> Result<String, String> {
        let rx = match self.receivers.lock() {
            Ok(mut receivers) => receivers.remove(question_id),
            Err(_) => return Err("question state is unavailable".into()),
        };
        match rx {
            Some(rx) => rx
                .await
                .unwrap_or_else(|_| Err("question was dismissed".into())),
            None => Err("no pending question with that id".into()),
        }
    }

    fn resolve(&self, question_id: &str, answer: String) -> Result<(), String> {
        let active = self
            .active_turn
            .lock()
            .map_err(|_| "question state is unavailable".to_string())?
            .is_some();
        if !active {
            return Err("no active turn for question".into());
        }
        let tx = self
            .senders
            .lock()
            .map_err(|_| "question state is unavailable".to_string())?
            .remove(question_id)
            .ok_or_else(|| "no pending question with that id".to_string())?;
        let _ = tx.send(Ok(answer));
        Ok(())
    }

    fn clear(&self) {
        if let Ok(mut senders) = self.senders.lock() {
            for (_, tx) in senders.drain() {
                let _ = tx.send(Err("question was dismissed".into()));
            }
        }
        if let Ok(mut receivers) = self.receivers.lock() {
            receivers.clear();
        }
        if let Ok(mut active) = self.active_turn.lock() {
            *active = None;
        }
    }
}

struct HubQuestioner {
    hub: Arc<QuestionHub>,
}

#[async_trait]
impl Questioner for HubQuestioner {
    async fn prepare(&self, question_id: &str) {
        self.hub.prepare(question_id);
    }

    async fn answer(&self, request: &QuestionRequest) -> Result<String, String> {
        self.hub.wait(&request.question_id).await
    }
}

struct DesktopProviderInteraction {
    approval_hub: Arc<ApprovalHub>,
    question_hub: Arc<QuestionHub>,
}

#[async_trait]
impl ProviderInteractionHost for DesktopProviderInteraction {
    async fn prepare_command_approval(&self, approval_id: &str) {
        self.approval_hub.prepare(approval_id);
    }

    async fn approve_command(&self, request: ProviderCommandRequest) -> bool {
        matches!(
            self.approval_hub.wait(&request.approval_id).await,
            ApprovalDecision::AllowOnce | ApprovalDecision::AllowSession
        )
    }

    async fn prepare_file_change_approval(&self, approval_id: &str) {
        self.approval_hub.prepare(approval_id);
    }

    async fn approve_file_change(&self, request: ProviderFileChangeRequest) -> bool {
        matches!(
            self.approval_hub.wait(&request.approval_id).await,
            ApprovalDecision::AllowOnce | ApprovalDecision::AllowSession
        )
    }

    async fn prepare_question(&self, question_id: &str) {
        self.question_hub.prepare(question_id);
    }

    async fn answer_question(&self, request: ProviderQuestionRequest) -> Option<Vec<String>> {
        self.question_hub
            .wait(&request.id)
            .await
            .ok()
            .map(|answer| vec![answer])
    }
}

/// The desktop opens in Auto: writes apply, allowlisted commands run, anything
/// else asks. Core's own default is Manual — see `ApprovalMode` — because a
/// library with no wired-up gate must not be permissive. Choosing the product
/// default here is the front-end's job.
const DESKTOP_DEFAULT_MODE: ApprovalMode = ApprovalMode::Auto;

/// The skill Plan mode runs. Blocking writes says what the model *cannot* do;
/// this says what it *should* do instead, and it is a markdown file so the
/// answer to "plan mode should say X" is an edit, not a release.
const PLAN_SKILL: &str = "plan";

struct AppState {
    sessions: SessionController,
    browser: Arc<BrowserHost>,
    login: Mutex<Option<LoginProcess>>,
    /// A gateway child started by this desktop process. An empty lease means
    /// the selected gateway was already running or was not local.
    gateway: Mutex<Option<GatewayLease>>,
    /// One coalescing transcript worker per open project. A background turn
    /// must keep its writer after the user navigates to another root.
    persist: Mutex<HashMap<PathBuf, PersistWorker>>,
    /// Preferred project root (explicit launch/folder choice / last-workspace).
    /// A usable launch directory is the strongest startup context; packaged
    /// launches normally start in the install directory, which is rejected by
    /// `usable_workspace` and therefore falls back to the remembered folder.
    workspace_root: Mutex<Option<PathBuf>>,
    /// The last working provider configuration. Keep its provider entry
    /// available when the destination is an ordinary folder with no zest.toml
    /// yet; an in-flight runtime may still be finishing in the old folder.
    workspace_config: Mutex<Option<Config>>,
    /// Mode + session grants. Outlives any one project so switching folders
    /// does not silently reset the user's chosen permission level.
    policy: Arc<Mutex<ApprovalPolicy>>,
    /// Serialize comment-preserving config read-modify-write operations made
    /// by Settings so two quick preset changes cannot overwrite each other.
    config_edit: Mutex<()>,
    /// In-memory summaries keep the sidebar from reparsing every full thread
    /// JSON file after each navigation or completed turn. File metadata is the
    /// invalidation signal, so changes made by another process are still seen.
    chat_summary_cache: Mutex<ChatSummaryCache>,
    /// Desktop-local project grouping. This is separate from the active
    /// repo/worktree boundary used by the agent.
    space_state: Mutex<SpaceState>,
    /// Project-scoped durable coordinator. Jobs remain in `.zest` when the
    /// active chat or window changes; this handle only owns live cancellation
    /// and bounded worker lanes.
    delegations: Arc<DelegationCoordinator>,
}

impl AppState {
    fn shutdown_gateway(&self) {
        if let Ok(mut lease) = self.gateway.lock() {
            if let Some(mut lease) = lease.take() {
                lease.shutdown();
            }
        }
    }
}

#[derive(Default)]
struct ChatSummaryCache {
    projects: HashMap<PathBuf, ProjectSummaryCache>,
}

#[derive(Default)]
struct ProjectSummaryCache {
    files: HashMap<String, CachedThreadSummary>,
}

#[derive(Clone)]
struct CachedThreadSummary {
    modified: Option<SystemTime>,
    length: u64,
    summary: ThreadSummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "ModelCapability.ts", rename_all = "camelCase")
)]
struct ModelCapability {
    id: String,
    efforts: Vec<String>,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    context_window: u64,
    supports_tools: bool,
    supports_vision: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "WorkspaceReview.ts", rename_all = "camelCase")
)]
struct WorkspaceReview {
    /// Short, user-facing result of the local review.
    summary: String,
    /// `git`, `not_git`, or `unavailable`.
    repository: String,
    /// Every changed path is counted; only the first few are returned for the
    /// compact Workbench panel.
    changed_files: Vec<String>,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    changed_file_count: usize,
    /// `clean`, `issues`, or `unavailable`.
    patch_check: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "PullRequestView.ts", rename_all = "camelCase")
)]
struct PullRequestView {
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    number: u64,
    title: String,
    url: String,
    state: String,
    is_draft: bool,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    additions: u64,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    deletions: u64,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    changed_files: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "GitContext.ts", rename_all = "camelCase")
)]
struct GitContextView {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    base_branch: Option<String>,
    branch_changed: bool,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    additions: u64,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    deletions: u64,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    changed_files: u64,
    /// `pull_request` when GitHub supplied the counts, otherwise `branch`.
    stats_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    pull_request: Option<PullRequestView>,
}

impl From<&PullRequestLink> for PullRequestView {
    fn from(link: &PullRequestLink) -> Self {
        Self {
            number: link.number,
            title: link.title.clone(),
            url: link.url.clone(),
            state: link.state.clone(),
            is_draft: link.is_draft,
            additions: link.additions,
            deletions: link.deletions,
            changed_files: link.changed_files,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "ThreadCheckpoint.ts", rename_all = "camelCase")
)]
struct ThreadCheckpointView {
    id: String,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    created_at: u64,
    label: String,
    message_count: usize,
    agent_message_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    anchor_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    preview: Option<String>,
    // Carries a `ThreadCheckpointKind` that has already been serialized, so the
    // Rust type is a String and ts-rs would widen it to `string`. Name the three
    // values it can hold instead: the UI switches on them, and `string` lets a
    // typo through that the union catches.
    #[cfg_attr(
        feature = "export-bindings",
        ts(type = "\"turn\" | \"compaction\" | \"manual\"")
    )]
    kind: String,
}

impl From<ThreadCheckpoint> for ThreadCheckpointView {
    fn from(checkpoint: ThreadCheckpoint) -> Self {
        Self {
            id: checkpoint.id,
            created_at: checkpoint.created_at,
            label: checkpoint.label,
            message_count: checkpoint.message_count,
            agent_message_count: checkpoint.agent_message_count,
            anchor_message_id: checkpoint.anchor_message_id,
            preview: checkpoint.preview,
            kind: match checkpoint.kind {
                ThreadCheckpointKind::Turn => "turn",
                ThreadCheckpointKind::Compaction => "compaction",
                ThreadCheckpointKind::Manual => "manual",
            }
            .into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "TurnRecovery.ts", rename_all = "camelCase")
)]
struct TurnRecoveryView {
    /// Lifecycle identity is useful for diagnostics and future provider resume,
    /// but the UI only needs the message id to prefill a retry.
    run_id: String,
    user_message_id: String,
}

impl From<&RecoverableRun> for TurnRecoveryView {
    fn from(recovery: &RecoverableRun) -> Self {
        Self {
            run_id: recovery.run_id.clone(),
            user_message_id: recovery.user_message_id.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "ProviderView.ts", rename_all = "camelCase")
)]
struct ProviderView {
    id: String,
    label: String,
    method: String,
    status_kind: String,
    status_label: String,
    detail: String,
    selectable: bool,
    can_connect: bool,
    /// Present in `zest.toml` / env fallback — Rust is authoritative for availability.
    configured: bool,
    default_model: String,
    models: Vec<ModelCapability>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "ExternalAgentView.ts", rename_all = "camelCase")
)]
struct ExternalAgentView {
    id: String,
    label: String,
    scope: String,
    mode: String,
    workspace: String,
    status_label: String,
    detail: String,
    configured: bool,
    mcp_allowed: bool,
    /// Empty means the worker CLI chooses its own configured/default model.
    model: String,
    /// CLI-owned model aliases shown by the built-in worker setup.
    models: Vec<String>,
    /// Presets can be enabled or removed from Settings. Other entries remain
    /// visible as read-only rows so manual configuration is discoverable.
    preset: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(
        export,
        export_to = "ExternalAgentCheckView.ts",
        rename_all = "camelCase"
    )
)]
struct ExternalAgentCheckView {
    available: bool,
    authenticated: Option<bool>,
    detail: String,
}

fn provider_view_from_slot(slot: &ProviderSlot, config: &Config) -> ProviderView {
    let configured_provider = config.providers.get(slot.id);
    let auth_status = match configured_provider {
        Some(ProviderConfig::ClaudeCode { .. }) => detect_claude_code(),
        Some(ProviderConfig::CodexCli { .. }) => detect_codex_cli(),
        _ => slot.status.clone(),
    };
    let (status_kind, status_label, detail) = match &auth_status {
        AuthStatus::Ready { account } => (
            "ready".into(),
            "Signed in".into(),
            account.clone().unwrap_or_else(|| slot.method.to_string()),
        ),
        AuthStatus::Unknown { reason } => {
            let detail = if reason.contains("could not verify this sign-in") {
                "Zest could not verify this sign-in.".into()
            } else {
                format!("Installed — {reason}")
            };
            ("unknown".into(), "Unverified".into(), detail)
        }
        AuthStatus::NotLoggedIn { fix } => (
            "not_logged_in".into(),
            "Not signed in".into(),
            if fix.starts_with("Connect") {
                fix.clone()
            } else {
                "Sign in to continue".into()
            },
        ),
        AuthStatus::Unconfigured => (
            "unconfigured".into(),
            "Not configured".into(),
            "No key set".into(),
        ),
    };

    let (configured, descriptor) = match configured_provider {
        Some(pc) => (true, descriptor_from_config(slot.id, pc)),
        None => (false, descriptor_for_picker_id(slot.id)),
    };
    let method = configured_provider
        .map(provider_method)
        .unwrap_or(slot.method);

    // Being signed in is not the same as being reachable. A vendor CLI can hold
    // a perfectly good session for a provider this project has no entry for,
    // and offering it as ready meant Continue failed *after* the click with
    // "not configured". Say so on the row instead.
    let (status_kind, status_label, detail) = if configured {
        (status_kind, status_label, detail)
    } else {
        (
            "unconfigured".to_string(),
            "Not configured".to_string(),
            match &auth_status {
                AuthStatus::Ready { .. } => {
                    "Signed in. Configure this provider in Settings.".into()
                }
                _ => "Configure this provider in Settings.".into(),
            },
        )
    };

    ProviderView {
        id: slot.id.to_string(),
        label: slot.label.to_string(),
        method: method.to_string(),
        status_kind,
        status_label,
        detail,
        // Both halves are required: a signed-in provider with no config cannot
        // serve a turn, and a configured provider with no sign-in cannot either.
        selectable: auth_status.selectable() && configured,
        can_connect: desktop_can_start_login(slot.id),
        configured,
        default_model: descriptor.default_model,
        models: descriptor
            .models
            .into_iter()
            .map(|m| ModelCapability {
                id: m.id,
                efforts: m.efforts,
                context_window: m.context_window,
                supports_tools: m.supports_tools,
                supports_vision: m.supports_vision,
            })
            .collect(),
    }
}

fn configured_provider_view(id: &str, config: &Config) -> ProviderView {
    let method = config
        .providers
        .get(id)
        .map(provider_method)
        .unwrap_or("API key");
    let descriptor = config
        .providers
        .get(id)
        .map(|entry| descriptor_from_config(id, entry))
        .unwrap_or_else(|| descriptor_for_picker_id(id));
    let (status_kind, status_label, detail) = match ProviderRegistry::from_config(config)
        .0
        .get(id)
        .map(|provider| provider.auth_status())
    {
        Some(AuthStatus::Ready { .. }) => ("ready", "Ready", format!("{method} provider")),
        Some(AuthStatus::Unknown { reason }) => ("unknown", "Unverified", reason),
        Some(AuthStatus::NotLoggedIn { fix }) => ("not_logged_in", "Not configured", fix),
        Some(AuthStatus::Unconfigured) | None => (
            "unconfigured",
            "Not configured",
            if method == "API key" {
                "Add an API key in Settings".to_string()
            } else {
                format!("Configure {method} to continue")
            },
        ),
    };
    ProviderView {
        id: id.to_string(),
        label: id
            .split(['-', '_'])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" "),
        method: method.into(),
        status_kind: status_kind.into(),
        status_label: status_label.into(),
        detail,
        selectable: status_kind == "ready",
        can_connect: false,
        configured: true,
        default_model: descriptor.default_model,
        models: descriptor
            .models
            .into_iter()
            .map(|model| ModelCapability {
                id: model.id,
                efforts: model.efforts,
                context_window: model.context_window,
                supports_tools: model.supports_tools,
                supports_vision: model.supports_vision,
            })
            .collect(),
    }
}

fn provider_method(config: &ProviderConfig) -> &'static str {
    match config {
        ProviderConfig::Anthropic { credential, .. } => {
            if credential.is_some() {
                "API key"
            } else {
                "Environment key"
            }
        }
        ProviderConfig::ClaudeCode { .. } => "Claude Code subscription",
        ProviderConfig::CodexCli { .. } => "Codex CLI subscription",
        ProviderConfig::Gateway { .. } => "Gateway",
        ProviderConfig::OpenaiCompatible {
            credential,
            api_key_env,
            ..
        } => match (credential.is_some(), api_key_env.is_some()) {
            (true, _) => "API key",
            (false, true) => "Environment key",
            (false, false) => "No authentication",
        },
    }
}

/// Desktop wire view of core `ToolMetadata` (ts-rs exportable).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "ToolMetaView.ts", rename_all = "snake_case")
)]
enum ToolMetaView {
    Delegation {
        provider_id: String,
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "export-bindings", ts(optional))]
        diff: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "export-bindings", ts(optional))]
        job_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "export-bindings", ts(optional))]
        stage: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "export-bindings", ts(optional))]
        attempt: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "export-bindings", ts(optional))]
        review_status: Option<String>,
    },
}

impl From<ToolMetadata> for ToolMetaView {
    fn from(meta: ToolMetadata) -> Self {
        match meta {
            ToolMetadata::Delegation {
                provider_id,
                model,
                diff,
                job_id,
                stage,
                attempt,
                review_status,
                usage: _,
            } => Self::Delegation {
                provider_id,
                model,
                diff,
                job_id,
                stage,
                attempt,
                review_status,
            },
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LoginStarted {
    browser_title: String,
    browser_body: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LoginStatus {
    state: String,
    detail: Option<String>,
}

/// A session without its transcript, for operations that do not change one.
///
/// Deliberately a sibling of [`SessionInfo`] rather than a field inside it:
/// `#[serde(flatten)]` and `ts-rs` do not agree about how to describe that, and
/// the wire shape is what the generated bindings gate protects. The duplicated
/// field list is the cost of both staying legible.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "SessionMeta.ts", rename_all = "camelCase")
)]
struct SessionMeta {
    session_id: String,
    provider: String,
    label: String,
    model: String,
    effort: String,
    root: String,
    is_free_chat: bool,
    thread_id: String,
    default_model: String,
    models: Vec<ModelCapability>,
    checkpoints: Vec<ThreadCheckpointView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    warning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    recovery: Option<TurnRecoveryView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "SessionInfo.ts", rename_all = "camelCase")
)]
struct SessionInfo {
    session_id: String,
    provider: String,
    label: String,
    model: String,
    effort: String,
    root: String,
    is_free_chat: bool,
    thread_id: String,
    /// Rust-authoritative catalogue for the active provider (UI may only add labels).
    default_model: String,
    models: Vec<ModelCapability>,
    checkpoints: Vec<ThreadCheckpointView>,
    /// UI projects these as `ChatMessage[]` (see `types.ts`); keep codegen free of StoredMessage.
    #[cfg_attr(feature = "export-bindings", ts(type = "unknown[]"))]
    messages: Vec<StoredMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    warning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    recovery: Option<TurnRecoveryView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadingDiffView {
    diff: String,
    summary: String,
    removed_lines: usize,
    folded_lines: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(
        export,
        export_to = "WorkspaceFileChange.ts",
        rename = "WorkspaceFileChange",
        rename_all = "camelCase"
    )
)]
struct WorkspaceFileChangeView {
    path: String,
    status: String,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    additions: u64,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    deletions: u64,
    binary: bool,
    sensitive: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(
        export,
        export_to = "WorkspaceChange.ts",
        rename = "WorkspaceChange",
        rename_all = "camelCase"
    )
)]
struct WorkspaceChangeView {
    change_id: String,
    repository: String,
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    base_commit: Option<String>,
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    base_branch: Option<String>,
    #[cfg_attr(feature = "export-bindings", ts(optional))]
    branch: Option<String>,
    changed_files: Vec<WorkspaceFileChangeView>,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    additions: u64,
    #[cfg_attr(feature = "export-bindings", ts(type = "number"))]
    deletions: u64,
    diff: String,
    truncated: bool,
    unavailable: bool,
}

impl From<zest_core::WorkspaceChangeSet> for WorkspaceChangeView {
    fn from(change: zest_core::WorkspaceChangeSet) -> Self {
        Self {
            change_id: change.change_id,
            repository: change.repository,
            base_commit: change.base_commit,
            base_branch: change.base_branch,
            branch: change.branch,
            changed_files: change
                .changed_files
                .into_iter()
                .map(|file| WorkspaceFileChangeView {
                    path: file.path,
                    status: file.status,
                    additions: file.additions,
                    deletions: file.deletions,
                    binary: file.binary,
                    sensitive: file.sensitive,
                })
                .collect(),
            additions: change.additions,
            deletions: change.deletions,
            diff: change.diff,
            truncated: change.truncated,
            unavailable: change.unavailable,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "ChatEvent.ts", rename_all = "snake_case")
)]
enum ChatEvent {
    User {
        session_id: String,
        thread_id: String,
        turn_id: String,
        message_id: String,
        text: String,
    },
    /// Empty streaming assistant row — emitted right after `User` so the UI can
    /// show Thinking… before the first provider delta.
    AssistantStart {
        session_id: String,
        thread_id: String,
        turn_id: String,
        message_id: String,
        /// Slash command that produced this turn, when one did. The UI titles
        /// the answer with it — Rust decides, because only Rust knows whether
        /// a leading `/token` matched a real skill.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
    },
    TextDelta {
        session_id: String,
        thread_id: String,
        turn_id: String,
        message_id: String,
        text: String,
    },
    ThinkingDelta {
        session_id: String,
        thread_id: String,
        turn_id: String,
        message_id: String,
        text: String,
    },
    /// Ephemeral activity from a provider-owned loop (for example Claude
    /// Code reading a file). It is deliberately not persisted as a Zest tool
    /// call and never enters the local tool executor.
    ProviderActivity {
        session_id: String,
        thread_id: String,
        turn_id: String,
        message_id: String,
        id: String,
        title: String,
        status: String,
    },
    ToolCallStart {
        session_id: String,
        thread_id: String,
        turn_id: String,
        message_id: String,
        name: String,
        id: String,
    },
    ToolCallUpdate {
        session_id: String,
        thread_id: String,
        turn_id: String,
        message_id: String,
        id: String,
        metadata: ToolMetaView,
    },
    ToolCallResult {
        session_id: String,
        thread_id: String,
        turn_id: String,
        message_id: String,
        name: String,
        id: String,
        summary: String,
        #[serde(rename = "isError")]
        is_error: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "export-bindings", ts(optional))]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "export-bindings", ts(optional))]
        diff: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "export-bindings", ts(optional))]
        metadata: Option<ToolMetaView>,
    },
    ApprovalNeeded {
        session_id: String,
        thread_id: String,
        turn_id: String,
        message_id: String,
        approval_id: String,
        tool_name: String,
        tool_call_id: String,
        risk: String,
        path: String,
        summary: String,
        diff: String,
    },
    QuestionNeeded {
        session_id: String,
        thread_id: String,
        turn_id: String,
        message_id: String,
        question_id: String,
        tool_call_id: String,
        prompt: String,
        choices: Vec<String>,
        multiple: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "export-bindings", ts(optional))]
        placeholder: Option<String>,
    },
    /// Emitted before every terminal event when the effective workspace patch
    /// changed during the turn. It is intentionally not applied to the
    /// transcript projection.
    WorkspaceChanged {
        session_id: String,
        thread_id: String,
        turn_id: String,
        change: WorkspaceChangeView,
    },
    Done {
        session_id: String,
        thread_id: String,
        turn_id: String,
        message_id: String,
    },
    Error {
        session_id: String,
        thread_id: String,
        turn_id: String,
        message_id: String,
        message: String,
        /// Provider to offer a Reconnect for, when the failure is one that only
        /// signing in again can fix. `None` for everything else — a Reconnect
        /// button on a rate limit would send the user through OAuth for nothing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reconnect_provider: Option<String>,
    },
    Cancelled {
        session_id: String,
        thread_id: String,
        turn_id: String,
        message_id: String,
    },
    Warning {
        session_id: String,
        thread_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "export-bindings", ts(optional))]
        turn_id: Option<String>,
        message: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopError {
    code: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<serde_json::Value>,
}

fn desktop_err(code: &str, message: impl Into<String>) -> String {
    desktop_err_with_details(code, message, None)
}

fn desktop_err_with_details(
    code: &str,
    message: impl Into<String>,
    details: Option<serde_json::Value>,
) -> String {
    let message = message.into();
    serde_json::to_string(&DesktopError {
        code: code.into(),
        message: message.clone(),
        details,
    })
    .unwrap_or(message)
}

fn map_session_err(e: SessionError) -> String {
    desktop_err(e.code(), e.message())
}

fn load_workspace_config(state: &AppState) -> Config {
    match resolve_workspace_root(state) {
        Ok(root) => {
            let mut config = Config::find(&root).unwrap_or_else(|_| Config::env_fallback());
            if can_inherit_workspace_config(&root) {
                merge_cached_providers(state, &mut config, None);
            }
            config
        }
        Err(_) => Config::env_fallback(),
    }
}

fn editable_config_path(state: &AppState) -> Result<PathBuf, String> {
    let root = resolve_workspace_root(state)?;
    if root.join(zest_core::config::CONFIG_FILE).is_file() {
        return Ok(root.join(zest_core::config::CONFIG_FILE));
    }
    zest_core::ensure_user_config()
        .map_err(|e| e.to_string())?
        .or_else(zest_core::user_config_path)
        .ok_or_else(|| "could not locate the user config directory".to_string())
}

fn clear_workspace_config_cache(state: &AppState) {
    if let Ok(mut cached) = state.workspace_config.lock() {
        *cached = None;
    }
}

/// A project config is an explicit boundary: it replaces the user config and
/// must not silently borrow a different provider table. A folder with no
/// config is different — it is the common case for an existing Zest install
/// opening a new codebase, so the active provider should follow the session.
fn can_inherit_workspace_config(root: &Path) -> bool {
    if root.join(zest_core::config::CONFIG_FILE).is_file() {
        return false;
    }
    !zest_core::user_config_path().is_some_and(|path| path.is_file())
}

fn merge_cached_providers(state: &AppState, config: &mut Config, only_provider: Option<&str>) {
    let cached = state
        .workspace_config
        .lock()
        .ok()
        .and_then(|guard| guard.clone());
    let Some(cached) = cached else { return };

    merge_provider_tables(config, &cached, only_provider);
}

fn merge_provider_tables(config: &mut Config, cached: &Config, only_provider: Option<&str>) {
    for (id, provider) in &cached.providers {
        if only_provider.is_some_and(|wanted| wanted != id) {
            continue;
        }
        config
            .providers
            .entry(id.clone())
            .or_insert_with(|| provider.clone());
    }
}

fn config_for_session(state: &AppState, root: &Path) -> Result<Config, String> {
    let mut config = Config::find(root).map_err(|e| e.to_string())?;
    if can_inherit_workspace_config(root) {
        merge_cached_providers(state, &mut config, None);
    }
    Ok(config)
}

fn config_for_free_chat(state: &AppState, root: &Path) -> Result<Config, String> {
    let mut config = Config::find(root).map_err(|e| e.to_string())?;
    // A free chat has no project-level zest.toml. Keep the provider table from
    // the workspace the user just left available, including providers that
    // exist only in that project's config.
    merge_cached_providers(state, &mut config, None);
    Ok(config)
}

fn remember_workspace_config(state: &AppState, config: &Config) {
    if let Ok(mut cached) = state.workspace_config.lock() {
        *cached = Some(config.clone());
    }
}

#[tauri::command]
fn list_providers(state: State<'_, AppState>) -> Vec<ProviderView> {
    let config = load_workspace_config(&state);
    // Provider status may inspect the gateway's credential store, but listing
    // providers must not provision a config or start a process. Adoption only
    // discovers a bundled sidecar for configured gateway rows.
    if config.providers.values().any(ProviderConfig::is_gateway) {
        zest_core::adopt_bundled_gateway();
    }
    let mut rows: Vec<ProviderView> = detect_all()
        .iter()
        .filter(|s| PICKER_IDS.contains(&s.id))
        .map(|s| provider_view_from_slot(s, &config))
        .collect();

    append_configured_direct_provider_views(&mut rows, &config);
    rows
}

fn append_configured_direct_provider_views(rows: &mut Vec<ProviderView>, config: &Config) {
    let existing: HashSet<String> = rows.iter().map(|row| row.id.clone()).collect();
    for (id, entry) in &config.providers {
        if existing.contains(id)
            || !matches!(
                entry,
                ProviderConfig::Anthropic { .. }
                    | ProviderConfig::ClaudeCode { .. }
                    | ProviderConfig::CodexCli { .. }
                    | ProviderConfig::OpenaiCompatible { .. }
            )
        {
            continue;
        }
        rows.push(configured_provider_view(id, config));
    }
}

#[tauri::command]
fn refresh_providers(state: State<'_, AppState>) -> Vec<ProviderView> {
    list_providers(state)
}

const EXTERNAL_AGENT_PRESETS: &[(&str, &str)] =
    &[("claude", "Claude Code"), ("gemini", "Gemini CLI")];

#[tauri::command]
fn list_external_agents(state: State<'_, AppState>) -> Vec<ExternalAgentView> {
    let _read_guard = state.config_edit.lock().ok();
    let config = load_workspace_config(&state);
    let scope = external_agent_scope(&state);
    let mut rows = EXTERNAL_AGENT_PRESETS
        .iter()
        .map(|(id, label)| {
            let configured = config.agents.get(*id);
            let editable = configured.is_none()
                || configured.is_some_and(|agent| external_agent_matches_preset(id, agent));
            external_agent_view(id, label, &scope, configured, editable)
        })
        .collect::<Vec<_>>();

    for id in config.agents.keys() {
        if EXTERNAL_AGENT_PRESETS
            .iter()
            .any(|(preset_id, _)| preset_id == id)
        {
            continue;
        }
        rows.push(external_agent_view(
            id,
            &title_case_id(id),
            &scope,
            config.agents.get(id),
            false,
        ));
    }
    rows
}

fn external_agent_view(
    id: &str,
    label: &str,
    scope: &str,
    config: Option<&zest_core::ExternalAgentConfig>,
    preset: bool,
) -> ExternalAgentView {
    let (default_mode, default_workspace) = zest_core::config_edit::external_agent_preset(id)
        .map(|input| (input.mode, input.workspace))
        .unwrap_or((ExternalAgentMode::Headless, ExternalWorkspace::Isolated));
    let mode = config.map(|agent| agent.mode).unwrap_or(default_mode);
    let workspace = config
        .map(|agent| agent.workspace)
        .unwrap_or(default_workspace);
    let configured = config.is_some();
    let model = config
        .and_then(|agent| agent.model.as_deref())
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or_default()
        .to_string();
    let mut models = if preset {
        zest_core::config_edit::external_agent_model_options(id)
            .iter()
            .map(|model| (*model).to_string())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if !model.is_empty() && !models.iter().any(|option| option == &model) {
        models.push(model.clone());
    }

    let (status_label, detail) = if !preset {
        (
            "Configured manually".into(),
            "Manage this worker in zest.toml.".into(),
        )
    } else if configured {
        (
            "Delegation enabled".into(),
            format!("Delegates through your {label} CLI session. Check the CLI before delegating."),
        )
    } else {
        (
            "Delegation off".into(),
            format!("Enable delegation to let Zest send bounded tasks to {label}."),
        )
    };

    ExternalAgentView {
        id: id.into(),
        label: label.into(),
        scope: scope.into(),
        mode: external_agent_mode_label(mode).into(),
        workspace: external_agent_workspace_label(workspace).into(),
        status_label,
        detail,
        configured,
        mcp_allowed: config.is_some_and(|agent| agent.allow_mcp),
        model,
        models,
        preset,
    }
}

fn external_agent_scope(state: &AppState) -> String {
    resolve_workspace_root(state)
        .ok()
        .map(|root| {
            if root.join(zest_core::config::CONFIG_FILE).is_file() {
                "Project zest.toml".to_string()
            } else {
                "User zest.toml".to_string()
            }
        })
        .unwrap_or_else(|| "Active zest.toml".to_string())
}

fn external_agent_mode_label(mode: ExternalAgentMode) -> &'static str {
    match mode {
        ExternalAgentMode::Headless => "Headless CLI",
        ExternalAgentMode::Acp => "CLI via ACP",
    }
}

fn external_agent_workspace_label(workspace: ExternalWorkspace) -> &'static str {
    match workspace {
        ExternalWorkspace::Isolated => "Isolated worktree",
        ExternalWorkspace::Current => "Current folder",
    }
}

fn title_case_id(id: &str) -> String {
    id.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[tauri::command]
fn set_external_agent(state: State<'_, AppState>, id: String, enabled: bool) -> Result<(), String> {
    let id = id.trim();
    let Some(preset) = zest_core::config_edit::external_agent_preset(id) else {
        return Err(
            "Only the built-in Claude Code and Gemini CLI presets can be changed here.".into(),
        );
    };
    let _edit_guard = state
        .config_edit
        .lock()
        .map_err(|_| "settings are busy; try again".to_string())?;
    let current = load_workspace_config(&state);
    if let Some(existing) = current.agents.get(id) {
        if !external_agent_matches_preset(id, existing) {
            return Err(format!(
                "{id} is customized in zest.toml; edit or remove that entry there first"
            ));
        }
    }
    let path = editable_config_path(&state)?;
    if enabled {
        zest_core::config_edit::upsert_external_agent(&path, &preset)?;
    } else {
        zest_core::config_edit::remove_external_agent(&path, id)?;
    }
    clear_workspace_config_cache(&state);
    Ok(())
}

#[tauri::command]
fn set_external_agent_mcp(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    let id = id.trim();
    let _edit_guard = state
        .config_edit
        .lock()
        .map_err(|_| "settings are busy; try again".to_string())?;
    let current = load_workspace_config(&state);
    let Some(existing) = current.agents.get(id) else {
        return Err("Enable this worker before allowing its MCP servers.".into());
    };
    if !external_agent_matches_preset(id, existing) {
        return Err(format!(
            "{id} is customized in zest.toml; edit or remove that entry there first"
        ));
    }
    let Some(preset) = zest_core::config_edit::external_agent_preset_with_model(
        id,
        enabled,
        existing.model.as_deref(),
    ) else {
        return Err(
            "MCP pass-through is available for the built-in Claude Code and Gemini CLI workers."
                .into(),
        );
    };
    let path = editable_config_path(&state)?;
    zest_core::config_edit::upsert_external_agent(&path, &preset)?;
    clear_workspace_config_cache(&state);
    Ok(())
}

#[tauri::command]
fn set_external_agent_model(
    state: State<'_, AppState>,
    id: String,
    model: Option<String>,
) -> Result<(), String> {
    let id = id.trim();
    let _edit_guard = state
        .config_edit
        .lock()
        .map_err(|_| "settings are busy; try again".to_string())?;
    let current = load_workspace_config(&state);
    let Some(existing) = current.agents.get(id) else {
        return Err("Enable this worker before choosing a model.".into());
    };
    if !external_agent_matches_preset(id, existing) {
        return Err(format!(
            "{id} is customized in zest.toml; edit or remove that entry there first"
        ));
    }
    let Some(preset) = zest_core::config_edit::external_agent_preset_with_model(
        id,
        existing.allow_mcp,
        model.as_deref(),
    ) else {
        return Err(
            "Worker model selection is available for the built-in Claude Code and Gemini CLI workers."
                .into(),
        );
    };
    let path = editable_config_path(&state)?;
    zest_core::config_edit::upsert_external_agent(&path, &preset)?;
    clear_workspace_config_cache(&state);
    Ok(())
}

fn external_agent_matches_preset(id: &str, agent: &zest_core::ExternalAgentConfig) -> bool {
    let matches_current = [false, true].into_iter().any(|allow_mcp| {
        let Some(preset) = zest_core::config_edit::external_agent_preset_with_model(
            id,
            allow_mcp,
            agent.model.as_deref(),
        ) else {
            return false;
        };
        agent.mode == preset.mode
            && agent.command == preset.command
            && agent.args == preset.args
            && agent.allow_mcp == preset.allow_mcp
            && agent.model == preset.model
            && agent.workspace == preset.workspace
            && agent.timeout_secs == preset.timeout_secs
    });
    matches_current || legacy_claude_preset_matches(id, agent)
}

fn legacy_claude_preset_matches(id: &str, agent: &zest_core::ExternalAgentConfig) -> bool {
    if id != "claude"
        || agent.mode != ExternalAgentMode::Headless
        || agent.command != "claude"
        || agent.allow_mcp
        || agent.model.is_some()
        || agent.workspace != ExternalWorkspace::Isolated
        || agent.timeout_secs != 900
    {
        return false;
    }

    let args = agent.args.iter().map(String::as_str).collect::<Vec<_>>();
    args == [
        "--print",
        "--output-format",
        "stream-json",
        "--strict-mcp-config",
        "{prompt}",
    ] || args
        == [
            "--print",
            "--verbose",
            "--output-format",
            "stream-json",
            "--strict-mcp-config",
            "{prompt}",
        ]
}

/// Check whether the configured CLI can start and, when the vendor exposes a
/// safe status command, whether its own session is authenticated. The command
/// output is parsed locally; account identity and credential details never
/// cross the desktop boundary.
#[tauri::command]
async fn check_external_agent(
    state: State<'_, AppState>,
    id: String,
) -> Result<ExternalAgentCheckView, String> {
    let config = load_workspace_config(&state);
    let agent = config
        .agents
        .get(id.trim())
        .ok_or_else(|| "Enable this worker before checking its CLI.".to_string())?;
    let root = resolve_workspace_root(&state)?;
    let mut command = Command::new(zest_core::resolve_program(&agent.command));
    command
        .arg("--version")
        .current_dir(&root)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    zest_core::prepare_external_command(&mut command);
    scrub_external_environment(&mut command);

    match tokio::time::timeout(Duration::from_secs(8), command.output()).await {
        Ok(Ok(output)) if output.status.success() => {}
        Ok(Ok(_)) => {
            return Ok(ExternalAgentCheckView {
                available: true,
                authenticated: None,
                detail: "CLI was found, but its version check failed.".to_string(),
            });
        }
        Ok(Err(_)) => {
            return Ok(ExternalAgentCheckView {
                available: false,
                authenticated: None,
                detail: "CLI not found on PATH.".to_string(),
            });
        }
        Err(_) => {
            return Ok(ExternalAgentCheckView {
                available: false,
                authenticated: None,
                detail: "CLI did not respond to a version check.".to_string(),
            });
        }
    };

    let (authenticated, detail) = if id.trim() == "claude" {
        let mut auth = Command::new(zest_core::resolve_program(&agent.command));
        auth.args(["auth", "status", "--json"])
            .current_dir(root)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        zest_core::prepare_external_command(&mut auth);
        scrub_external_environment(&mut auth);

        match tokio::time::timeout(Duration::from_secs(8), auth.output()).await {
            Ok(Ok(output)) if output.status.success() => {
                let authenticated = serde_json::from_slice::<serde_json::Value>(&output.stdout)
                    .ok()
                    .and_then(|value| value.get("loggedIn").and_then(|value| value.as_bool()));
                let detail = match authenticated {
                    Some(true) => "CLI available. Signed in to Claude.".to_string(),
                    Some(false) => "CLI available, but Claude is not signed in.".to_string(),
                    None => "CLI available. Sign-in status could not be verified.".to_string(),
                };
                (authenticated, detail)
            }
            Ok(Ok(_)) => (
                None,
                "CLI available. Sign-in status could not be verified.".to_string(),
            ),
            Ok(Err(_)) => (
                None,
                "CLI available, but its sign-in status could not be checked.".to_string(),
            ),
            Err(_) => (
                None,
                "CLI available, but its sign-in check timed out.".to_string(),
            ),
        }
    } else {
        (
            None,
            "CLI available. This worker does not expose a sign-in status check.".to_string(),
        )
    };

    Ok(ExternalAgentCheckView {
        available: true,
        authenticated,
        detail,
    })
}

fn scrub_external_environment(command: &mut Command) {
    for (name, _) in std::env::vars() {
        let upper = name.to_ascii_uppercase();
        if ["KEY", "TOKEN", "SECRET", "PASSWORD", "CREDENTIAL"]
            .iter()
            .any(|marker| upper.contains(marker))
        {
            command.env_remove(name);
        }
    }
}

#[tauri::command]
fn set_provider_key(state: State<'_, AppState>, id: String, key: String) -> Result<(), String> {
    let config = load_workspace_config(&state);
    let credential = match config.providers.get(&id) {
        Some(ProviderConfig::Anthropic {
            credential: Some(credential),
            ..
        })
        | Some(ProviderConfig::OpenaiCompatible {
            credential: Some(credential),
            ..
        }) => credential,
        Some(ProviderConfig::Anthropic { .. }) | Some(ProviderConfig::OpenaiCompatible { .. }) => {
            return Err("This provider gets its API key from an environment variable.".into())
        }
        Some(_) => return Err("This provider does not accept an API key.".to_string()),
        None => return Err("This provider does not accept an API key.".to_string()),
    };
    if key.trim().is_empty() {
        return Err("API key cannot be empty".into());
    }
    zest_core::credentials::set(credential, &key)
}

#[tauri::command]
fn delete_provider_key(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let config = load_workspace_config(&state);
    let credential = match config.providers.get(&id) {
        Some(ProviderConfig::Anthropic {
            credential: Some(credential),
            ..
        })
        | Some(ProviderConfig::OpenaiCompatible {
            credential: Some(credential),
            ..
        }) => credential,
        Some(_) => return Err("This provider does not accept an API key.".to_string()),
        None => return Err("This provider does not accept an API key.".to_string()),
    };
    zest_core::credentials::delete(credential)
}

#[tauri::command]
fn provider_key_present(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    let config = load_workspace_config(&state);
    let credential = match config.providers.get(&id) {
        Some(ProviderConfig::Anthropic {
            credential: Some(credential),
            ..
        })
        | Some(ProviderConfig::OpenaiCompatible {
            credential: Some(credential),
            ..
        }) => credential,
        Some(_) => return Err("This provider does not accept an API key.".to_string()),
        None => return Err("This provider does not accept an API key.".to_string()),
    };
    zest_core::credentials::present(credential)
}

#[tauri::command]
fn configure_api_provider(
    state: State<'_, AppState>,
    id: String,
    base_url: String,
    model: String,
    models: Vec<String>,
    credential: String,
    key: String,
) -> Result<(), String> {
    if key.trim().is_empty() {
        return Err("API key is required".into());
    }
    let _edit_guard = state
        .config_edit
        .lock()
        .map_err(|_| "settings are busy; try again".to_string())?;
    let path = editable_config_path(&state)?;
    zest_core::config_edit::add_openai_provider(
        &path,
        &zest_core::config_edit::OpenAiProviderInput {
            id: id.clone(),
            base_url,
            model,
            models,
            credential: credential.clone(),
        },
    )?;
    zest_core::credentials::set(credential.trim(), key.trim())?;
    clear_workspace_config_cache(&state);
    Ok(())
}

#[tauri::command]
fn configure_anthropic_provider(
    state: State<'_, AppState>,
    id: String,
    model: String,
    credential: String,
    key: String,
) -> Result<(), String> {
    if key.trim().is_empty() {
        return Err("API key is required".into());
    }
    let _edit_guard = state
        .config_edit
        .lock()
        .map_err(|_| "settings are busy; try again".to_string())?;
    let path = editable_config_path(&state)?;
    zest_core::config_edit::add_anthropic_provider(
        &path,
        &zest_core::config_edit::AnthropicProviderInput {
            id: id.clone(),
            model,
            credential: credential.clone(),
        },
    )?;
    zest_core::credentials::set(credential.trim(), key.trim())?;
    clear_workspace_config_cache(&state);
    Ok(())
}

#[tauri::command]
fn configure_claude_code_provider(
    state: State<'_, AppState>,
    id: String,
    model: String,
) -> Result<(), String> {
    let _edit_guard = state
        .config_edit
        .lock()
        .map_err(|_| "settings are busy; try again".to_string())?;
    let path = editable_config_path(&state)?;
    zest_core::config_edit::add_claude_code_provider(
        &path,
        &zest_core::config_edit::ClaudeCodeProviderInput {
            id,
            command: "claude".into(),
            model,
            models: vec!["sonnet".into(), "opus".into(), "haiku".into()],
            allow_mcp: false,
            // Not `accept_edits`: that auto-approves inside the CLI before zest
            // is consulted, so edits would land with no approval card and no
            // diff. The provider downgrades it anyway — writing it here would
            // only mislead someone reading their own zest.toml.
            permission_mode: zest_core::ClaudeCodePermissionMode::Default,
            timeout_secs: 900,
        },
    )?;
    clear_workspace_config_cache(&state);
    Ok(())
}

/// Open the project-local configuration in the user's default editor.
///
/// Provider ownership is deliberately configured in `zest.toml`; this keeps
/// the recovery dialog from silently changing a project's provider table.
#[tauri::command]
fn open_project_config(root: String) -> Result<(), String> {
    let root = canonicalize_dir(PathBuf::from(root.trim()))?;
    let path = root.join(zest_core::config::CONFIG_FILE);
    if !path.is_file() {
        return Err(
            "This project has no zest.toml yet. Add the provider to its project configuration first."
                .into(),
        );
    }

    open_path_in_editor(&path).map_err(|e| format!("could not open project configuration: {e}"))
}

/// Hand a file to whatever the OS opens it with.
///
/// Shared by every "edit this yourself" affordance, so a new one cannot pick up
/// a platform arm the others have and get it subtly wrong.
fn open_path_in_editor(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        // The empty argument is `start`'s title parameter. Without it a quoted
        // path is consumed as the window title and nothing opens.
        StdCommand::new("cmd")
            .args(["/C", "start", ""])
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        StdCommand::new("open")
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        StdCommand::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let _ = path;
        return Err("opening a file is not supported on this platform".into());
    }

    Ok(())
}

#[tauri::command]
fn usage_snapshot() -> UsageSnapshot {
    Ledger::load().snapshot()
}

#[tauri::command]
async fn provider_quota(state: State<'_, AppState>) -> Result<ProviderQuotaSnapshot, String> {
    let config = load_workspace_config(&state);
    Ok(zest_core::fetch_provider_quotas(&config).await)
}

#[tauri::command]
fn list_plugins() -> Vec<PluginView> {
    plugins::list()
}

#[tauri::command]
fn open_plugins_folder() -> Result<(), String> {
    let path = plugins::ensure_plugin_folder()?;
    open_path_in_editor(&path).map_err(|error| format!("Could not open add-ons folder: {error}"))
}

#[tauri::command]
fn set_plugin_enabled(id: String, enabled: bool) -> Result<Vec<PluginView>, String> {
    plugins::set_enabled(&id, enabled)
}

#[tauri::command]
async fn now_playing() -> Result<NowPlayingView, String> {
    tauri::async_runtime::spawn_blocking(plugins::now_playing)
        .await
        .map_err(|_| "Could not read the music.".to_string())
}

#[tauri::command]
async fn control_now_playing(action: String) -> Result<NowPlayingView, String> {
    tauri::async_runtime::spawn_blocking(move || plugins::control(&action))
        .await
        .map_err(|_| "Could not change the music.".to_string())?
}

#[tauri::command]
async fn set_now_playing_volume(volume_percent: f64) -> Result<NowPlayingView, String> {
    tauri::async_runtime::spawn_blocking(move || plugins::set_volume(volume_percent))
        .await
        .map_err(|_| "Could not change the volume.".to_string())?
}

/// Spend, tokens, and cost for the last `days` local days.
///
/// Read fresh from disk on every call rather than served from the session's
/// in-memory ledger: a second Zest window, or a CLI run in another terminal,
/// writes the same file, and a usage screen that silently omits them would be
/// exactly the kind of partial figure this ledger exists to avoid.
///
/// The price book is loaded per call for the same reason — someone correcting a
/// rate expects the refresh button to show it, not the next app launch.
///
/// Runs on a blocking thread because the transcript scan walks and reads files:
/// a few hundred milliseconds is fine, but not on the thread serving the UI.
#[tauri::command]
async fn usage_report(days: u32) -> Result<UsageReport, String> {
    // Clamped rather than trusted: `days` comes from the webview, and a wild
    // value would allocate one series point per day of it.
    let days = days.clamp(1, DAILY_RETENTION_DAYS as u32);
    tauri::async_runtime::spawn_blocking(move || {
        let scan = zest_core::transcripts::scan(days);
        Ledger::load().report(days, &Prices::load(), Some(&scan))
    })
    .await
    .map_err(|e| format!("could not read usage: {e}"))
}

/// Fetch the published rate table if the cached copy is due for renewal.
///
/// Deliberately its own command rather than something `usage_report` does. The
/// report must stay instant and must never fail because GitHub is having a bad
/// morning; this can take a network round trip, so the front end calls it
/// alongside the report and re-reads the report only if the rates actually
/// moved. `force` is the Refresh button saying "I know it is not due yet".
#[tauri::command]
async fn refresh_rates(force: bool) -> RatesStatus {
    let catalog = zest_core::rates::refresh(force).await;
    RatesStatus {
        catalog_models: catalog.len(),
        overrides: Prices::load().models.len(),
        fetched_at: catalog.fetched_at(),
        stale: catalog.is_stale(),
        source_url: zest_core::DEFAULT_RATES_URL.to_string(),
    }
}

/// Open the price book in the OS text editor.
///
/// The rates are the user's to correct, which is only true if getting to them
/// takes one click rather than a hunt through the data directory.
#[tauri::command]
fn open_prices_file() -> Result<(), String> {
    let prices = Prices::load();
    let path = prices
        .path()
        .ok_or_else(|| "there is no price book on this platform".to_string())?;
    open_path_in_editor(path)
}

/// Tell core which day it is for this user.
///
/// The webview is the only part of Zest that knows the machine's timezone, and
/// every day boundary — streaks, heatmap cells, which bucket a turn lands in —
/// depends on it. Called at startup, before anything is recorded.
#[tauri::command]
fn set_local_offset(minutes: i32) {
    zest_core::usage::set_local_offset_minutes(minutes);
}

/// Activity statistics across every project Zest knows about.
///
/// Chats come from thread files, so this is retroactive; tokens come from the
/// ledger's daily buckets, which only exist from when metering landed. The two
/// reaches are kept distinct in the payload rather than blended.
#[tauri::command]
fn profile_stats(state: State<'_, AppState>) -> Result<ProfileStats, String> {
    let mut roots = load_known_workspaces();
    if let Ok(active) = resolve_workspace_root(&state) {
        if !roots.iter().any(|p| p == &active) {
            roots.insert(0, active);
        }
    }

    let mut chats = Vec::new();
    for root in roots {
        if !root.is_dir() {
            continue;
        }
        // A project that has been moved or deleted is skipped, not fatal: a
        // profile is a summary, and one missing folder should not blank it.
        let Ok(store) = open_store(&root) else {
            continue;
        };
        for thread in store.list().unwrap_or_default() {
            chats.push(ChatFacts {
                created_at: thread.created_at,
                updated_at: thread.updated_at,
                message_count: thread.message_count,
            });
        }
    }

    let ledger = Ledger::load();
    let (tokens, requests) = ledger.lifetime();
    let today = zest_core::usage::local_day_number(now_secs());
    Ok(derive_profile_stats(
        &chats,
        ledger.daily(),
        tokens,
        requests,
        today,
    ))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Send one minimal turn to prove the provider can actually serve.
///
/// A credentials file on disk is not a working session — the gateway can hold
/// an account it has put in cooldown, and that never shows up locally. Called
/// after a sign-in and again before opening a gateway chat.
#[tauri::command]
async fn verify_provider(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let root = resolve_workspace_root(&state)?;
    let config = config_for_session(&state, &root)?;
    let label = detect_all()
        .into_iter()
        .find(|s| s.id == id)
        .map(|s| s.label.to_string())
        .unwrap_or_else(|| id.clone());

    // This is now the only place the "Connect again" wording is produced, because
    // opening a chat no longer probes. It stays tied to `needs_reconnect` so an
    // unreachable gateway is never reported as a credential problem — telling
    // someone to re-run OAuth cannot start a process that is not running.
    prove_provider_serves(&state, &config, &id)
        .await
        .map_err(|failure| {
            if failure.needs_reconnect() && desktop_can_start_login(&id) {
                format!("{label} needs to be reconnected. Try again.")
            } else {
                failure.user_message_for_provider(&id)
            }
        })
}

/// Why a provider could not be proven able to serve.
///
/// Kept typed rather than pre-formatted so callers can still tell a credential
/// problem from everything else. Deciding that by matching on a message string
/// is how "the gateway is not running" came to be reported as a bad session.
enum ProbeFailure {
    /// Configuration, workspace, or gateway startup — no turn was attempted, so
    /// this says nothing about the account.
    Setup(String),
    /// A real turn was attempted and failed.
    Turn(HarnessError),
}

impl ProbeFailure {
    fn user_message(&self) -> String {
        match self {
            Self::Setup(message) => message.clone(),
            Self::Turn(err) => format_turn_error(err),
        }
    }

    fn user_message_for_provider(&self, provider_id: &str) -> String {
        match self {
            Self::Setup(message) => message.clone(),
            Self::Turn(err) => format_turn_error_for_provider(err, provider_id),
        }
    }

    /// Whether signing in again is actually the fix.
    fn needs_reconnect(&self) -> bool {
        matches!(self, Self::Turn(err) if err.is_auth_problem())
    }
}

/// Prove the provider can actually serve: gateway up **and** account working.
///
/// Both halves, for an explicit Connect or verify. Opening a chat deliberately
/// runs only the first half — see [`ensure_gateway_ready`].
async fn prove_provider_serves(
    state: &AppState,
    config: &Config,
    id: &str,
) -> Result<(), ProbeFailure> {
    ensure_gateway_ready(&state.gateway, config, id).await?;
    probe_provider(config, id).await
}

/// Make the local gateway available, without spending a turn to find out whether
/// the account behind it works.
///
/// Cheap and local — a TCP check, and a process spawn when nothing answers. This
/// is the half that has to happen before a chat opens, because every turn needs
/// the port open; proving the *account* is a network round trip that costs tokens
/// and belongs behind the UI rather than in front of it.
async fn ensure_gateway_ready(
    gateway: &Mutex<Option<GatewayLease>>,
    config: &Config,
    id: &str,
) -> Result<(), ProbeFailure> {
    // Start the local gateway rather than probing a port nothing is listening on.
    // Its being down is the ordinary state after a reboot, not a user error, and
    // Zest launches this same binary to sign in — so it can launch it to serve.
    if let Some(base_url) = local_gateway_url(config, id) {
        let start = ensure_gateway_running(&base_url).await;
        if matches!(start.state, GatewayState::Unavailable(_)) {
            return Err(ProbeFailure::Setup(
                "Zest could not start this provider. Try again.".into(),
            ));
        }
        if start.state == GatewayState::Listening && start.lease.is_owned() {
            let mut owned = gateway
                .lock()
                .map_err(|_| ProbeFailure::Setup("Gateway state is unavailable.".into()))?;
            // A lease may remain after its child exits unexpectedly. Replace
            // it with the freshly verified child; dropping the old lease only
            // targets the old retained handle.
            let old = owned.replace(start.lease);
            drop(old);
        }
    }
    Ok(())
}

/// Send one minimal turn to find out whether the account can serve.
///
/// A credentials file on disk is not a working session: a gateway can hold an
/// account it has put in cooldown, and that never shows up locally.
async fn probe_provider(config: &Config, id: &str) -> Result<(), ProbeFailure> {
    zest_core::load_env();
    let (registry, skipped) = ProviderRegistry::from_config(config);

    let provider = registry.get(id).ok_or_else(|| {
        ProbeFailure::Setup(if skipped.iter().any(|s| s.id == id) {
            "Could not load this provider. Check its configuration and try again.".into()
        } else {
            "Configure this provider before continuing.".into()
        })
    })?;

    if matches!(provider.auth_status(), AuthStatus::Unconfigured) {
        return Err(ProbeFailure::Setup(
            "Add a valid API key for this provider before continuing.".into(),
        ));
    }

    let model = provider.default_model().to_string();
    probe(provider.as_ref(), &model)
        .await
        .map_err(ProbeFailure::Turn)
}

/// The `base_url` of a gateway-kind provider, for gateway supervision.
///
/// `None` for a native provider: it has no local process behind it, so there is
/// nothing to start and nothing to blame for being down.
fn local_gateway_url(config: &Config, id: &str) -> Option<String> {
    match config.providers.get(id)? {
        ProviderConfig::Gateway { base_url, .. } => Some(base_url.clone()),
        ProviderConfig::Anthropic { .. }
        | ProviderConfig::ClaudeCode { .. }
        | ProviderConfig::CodexCli { .. }
        | ProviderConfig::OpenaiCompatible { .. } => None,
    }
}

#[tauri::command]
fn start_login(state: State<'_, AppState>, id: String) -> Result<LoginStarted, String> {
    if !desktop_can_start_login(&id) {
        return Err(match id.as_str() {
            "claude" => {
                "Claude Code sign-in is managed by the Claude Code CLI. Run `claude login`, then enable Claude Code as a parent provider.".into()
            }
            "antigravity" => {
                "Gemini sign-in is managed by the Gemini CLI. Sign in there, then enable Gemini CLI under Settings → External workers.".into()
            }
            _ => "This provider has no sign-in flow managed by Zest. Configure it in Settings.".into(),
        });
    }

    let mut active = state
        .login
        .lock()
        .map_err(|_| "login state lock poisoned".to_string())?;
    if let Some(process) = active.as_mut() {
        if process
            .try_wait()
            .map_err(|e| format!("could not inspect the existing sign-in: {e}"))?
            .is_none()
        {
            return Err("A sign-in is already in progress. Finish it or cancel it first.".into());
        }
        *active = None;
    }

    let native_codex = matches!(
        load_workspace_config(&state).providers.get(&id),
        Some(ProviderConfig::CodexCli { .. })
    );
    let process = if id == "claude" {
        core_start_claude_code_login()?
    } else if native_codex {
        core_start_codex_cli_login()?
    } else {
        core_start_login(&id)?
    };
    let spawn = &process.spawn;
    let started = LoginStarted {
        browser_title: spawn.browser_title.to_string(),
        browser_body: spawn.browser_body.to_string(),
    };
    *active = Some(process);
    Ok(started)
}

#[tauri::command]
fn login_status(state: State<'_, AppState>) -> Result<LoginStatus, String> {
    let mut active = state
        .login
        .lock()
        .map_err(|_| "login state lock poisoned".to_string())?;
    let Some(process) = active.as_mut() else {
        return Ok(LoginStatus {
            state: "idle".into(),
            detail: None,
        });
    };

    let Some(_status) = process
        .try_wait()
        .map_err(|e| format!("could not inspect the sign-in process: {e}"))?
    else {
        return Ok(LoginStatus {
            state: "running".into(),
            detail: None,
        });
    };

    let detail = "The sign-in did not finish. Try again.".to_string();
    *active = None;
    Ok(LoginStatus {
        state: "exited".into(),
        detail: Some(detail),
    })
}

#[tauri::command]
fn cancel_login(state: State<'_, AppState>) -> Result<(), String> {
    let mut active = state
        .login
        .lock()
        .map_err(|_| "login state lock poisoned".to_string())?;
    if let Some(process) = active.as_mut() {
        process
            .kill()
            .map_err(|e| format!("could not stop the sign-in process: {e}"))?;
    }
    *active = None;
    Ok(())
}

/// Marker prefix on every "this folder will not work as a project" failure.
///
/// The UI matches on this token instead of sniffing the OS error text. Windows
/// says "Access is denied.", POSIX says "Permission denied", and matching
/// neither is exactly how a first-run install-directory failure used to surface
/// in the picker as an unattributed "Something went wrong. Try again."
const WORKSPACE_NOT_WRITABLE: &str = "workspace_not_writable";

fn canonicalize_dir(path: PathBuf) -> Result<PathBuf, String> {
    if !path.is_dir() {
        return Err(format!("not a directory: {}", path.display()));
    }
    path.canonicalize().or(Ok(path))
}

/// Can Zest actually create `.zest/` here?
///
/// `is_dir` is not enough, and neither is any metadata flag: on Windows the
/// answer lives in an ACL, and read-only mounts and full disks fail the same
/// way. Writing a real file is the only probe that agrees with what the session
/// is about to attempt.
fn dir_is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".zest-write-probe-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Where the running executable lives — never a project folder.
///
/// A packaged install is handed its own install directory as the working
/// directory, and that directory is read-only without elevation on every
/// platform Zest ships to (`C:\Program Files\Zest`, `/Applications`, `/usr/lib`).
/// Rejecting it by location as well as by writability keeps an elevated or
/// misconfigured install from quietly filling the program folder with chat
/// history.
fn install_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    canonicalize_dir(exe.parent()?.to_path_buf()).ok()
}

fn is_install_dir(path: &Path) -> bool {
    install_dir().is_some_and(|dir| path.starts_with(&dir))
}

/// A candidate is a workspace only if Zest can keep its state there.
fn usable_workspace(path: PathBuf) -> Option<PathBuf> {
    let dir = canonicalize_dir(path).ok()?;
    if is_install_dir(&dir) || !dir_is_writable(&dir) {
        return None;
    }
    Some(dir)
}

fn cwd_workspace() -> Option<PathBuf> {
    usable_workspace(std::env::current_dir().ok()?)
}

/// Select the initial project without letting a stale remembered workspace
/// override an explicit directory supplied by the process that launched Zest.
///
/// Development launches can carry the launched directory in the process
/// working directory. Packaged launches are commonly started in Zest's own
/// install directory; that path is rejected by `usable_workspace`, so normal
/// last-workspace behavior remains intact there.
fn choose_initial_workspace(
    launch: Option<PathBuf>,
    remembered: Option<PathBuf>,
    fallback: Option<PathBuf>,
) -> Option<PathBuf> {
    launch.or(remembered).or(fallback)
}

fn initial_workspace_root() -> Option<PathBuf> {
    choose_initial_workspace(
        cwd_workspace(),
        load_persisted_workspace().and_then(usable_workspace),
        default_workspace(),
    )
}

/// The folder first run falls back to when the working directory is unusable.
///
/// Documents is where someone who did not choose a project expects to find
/// their own files, and a dedicated subfolder keeps `.zest/` out of its top
/// level. Created on demand so the very first launch has somewhere to land
/// instead of dead-ending on the picker.
fn default_workspace() -> Option<PathBuf> {
    let base = dirs::document_dir().or_else(dirs::home_dir)?;
    let root = base.join("Zest");
    if !root.is_dir() {
        std::fs::create_dir_all(&root).ok()?;
    }
    if !dir_is_writable(&root) {
        return None;
    }
    canonicalize_dir(root).ok()
}

fn no_writable_workspace_error() -> String {
    format!(
        "{WORKSPACE_NOT_WRITABLE}: Zest could not find a project folder it is allowed to write to. \
         Use Open to choose one inside your user account, such as a folder under Documents."
    )
}

/// Explain a storage failure in terms of the folder, not the OS error.
///
/// Every caller here is about to create something under `<root>/.zest`, so when
/// the root is not writable that is the whole story and the raw errno adds
/// nothing a user can act on.
fn workspace_write_error(root: &Path, error: impl std::fmt::Display) -> String {
    if !dir_is_writable(root) {
        return format!(
            "{WORKSPACE_NOT_WRITABLE}: Zest cannot save chats in {}. \
             Use Open to choose a folder you own, such as one under Documents.",
            display_path(root)
        );
    }
    error.to_string()
}

fn load_persisted_workspace() -> Option<PathBuf> {
    let path = dirs::config_dir()?.join("zest").join("last-workspace");
    let value = std::fs::read_to_string(path).ok()?;
    let value = value.trim().to_string();
    if value.is_empty() {
        return None;
    }
    let candidate = PathBuf::from(value);
    canonicalize_dir(candidate).ok()
}

fn persist_workspace(root: &Path) -> Result<(), String> {
    let path = zest_config_dir()?.join("last-workspace");
    std::fs::write(&path, display_path(root)).map_err(|e| e.to_string())?;
    remember_workspace(root);
    Ok(())
}

const KNOWN_WORKSPACES_FILE: &str = "known-workspaces.json";
const MAX_KNOWN_WORKSPACES: usize = 40;

fn known_workspaces_path() -> Result<PathBuf, String> {
    Ok(zest_config_dir()?.join(KNOWN_WORKSPACES_FILE))
}

fn load_known_workspaces() -> Vec<PathBuf> {
    let Ok(path) = known_workspaces_path() else {
        return Vec::new();
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(list) = serde_json::from_str::<Vec<String>>(&raw) else {
        return Vec::new();
    };
    list.into_iter()
        .filter_map(|s| {
            let p = PathBuf::from(s.trim());
            if p.as_os_str().is_empty() {
                return None;
            }
            canonicalize_dir(p).ok()
        })
        .collect()
}

fn write_known_workspaces(list: &[PathBuf]) -> Result<(), String> {
    let display: Vec<String> = list.iter().map(|path| display_path(path)).collect();
    let raw = serde_json::to_string_pretty(&display).map_err(|error| error.to_string())?;
    std::fs::write(known_workspaces_path()?, raw).map_err(|error| error.to_string())
}

fn require_known_workspace(path: &Path) -> Result<PathBuf, String> {
    let root = canonicalize_dir(path.to_path_buf())?;
    let root_key = display_path(&root);
    if load_known_workspaces()
        .iter()
        .any(|known| display_path(known) == root_key)
    {
        Ok(root)
    } else {
        Err(format!(
            "That folder is not a known Zest workspace: {root_key}"
        ))
    }
}

fn remember_workspace(root: &Path) {
    let Ok(root) = canonicalize_dir(root.to_path_buf()) else {
        return;
    };
    let mut list = load_known_workspaces();
    list.retain(|p| p != &root);
    list.insert(0, root);
    list.truncate(MAX_KNOWN_WORKSPACES);
    let _ = write_known_workspaces(&list);
}

fn clear_persisted_workspace(root_key: &str) -> Result<Option<String>, String> {
    let path = zest_config_dir()?.join("last-workspace");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let candidate = PathBuf::from(raw.trim());
    if candidate.as_os_str().is_empty() {
        return Ok(None);
    }
    let matches = display_path(&candidate) == root_key
        || canonicalize_dir(candidate)
            .ok()
            .is_some_and(|path| display_path(&path) == root_key);
    if matches {
        std::fs::remove_file(path).map_err(|error| error.to_string())?;
        return Ok(Some(raw));
    }
    Ok(None)
}

fn restore_persisted_workspace(previous: Option<&str>) -> Result<(), String> {
    let path = zest_config_dir()?.join("last-workspace");
    match previous {
        Some(raw) => std::fs::write(path, raw).map_err(|error| error.to_string()),
        None => Ok(()),
    }
}

fn remove_known_workspace_metadata(
    state: &AppState,
    root_key: &str,
    known_roots: &[PathBuf],
) -> Result<(), String> {
    if !known_roots
        .iter()
        .any(|root| display_path(root) == root_key)
    {
        return Err(format!(
            "That folder is not a known Zest workspace: {root_key}"
        ));
    }

    let mut remaining = known_roots.to_vec();
    remaining.retain(|root| display_path(root) != root_key);
    let previous_persisted_workspace = clear_persisted_workspace(root_key)?;
    if let Err(error) = write_known_workspaces(&remaining) {
        let _ = restore_persisted_workspace(previous_persisted_workspace.as_deref());
        return Err(error);
    }

    let mut spaces = state
        .space_state
        .lock()
        .map_err(|_| "Space state lock poisoned.".to_string())?;
    let previous = spaces.clone();
    spaces.forget_project(root_key);
    if let Err(error) = save_space_state(&spaces) {
        *spaces = previous;
        let _ = write_known_workspaces(known_roots);
        let _ = restore_persisted_workspace(previous_persisted_workspace.as_deref());
        return Err(error);
    }
    drop(spaces);

    if let Ok(mut cache) = state.chat_summary_cache.lock() {
        cache
            .projects
            .retain(|root, _| display_path(root) != root_key);
    }
    Ok(())
}

fn workspace_removal_target(
    state: &AppState,
    project_path: &str,
) -> Result<(PathBuf, String, Vec<PathBuf>), String> {
    let root = require_known_workspace(Path::new(project_path.trim()))?;
    let root_key = display_path(&root);
    let active_root = state.sessions.active_root().map_err(map_session_err)?;
    if active_root
        .as_ref()
        .is_some_and(|active| display_path(active) == root_key)
    {
        return Err("Switch to another project before removing the active workspace.".to_string());
    }
    let known_roots = load_known_workspaces();
    if !known_roots
        .iter()
        .any(|known| display_path(known) == root_key)
    {
        return Err(format!(
            "That folder is not a known Zest workspace: {root_key}"
        ));
    }
    Ok((root, root_key, known_roots))
}

fn space_snapshot(state: &AppState) -> Result<SpacesSnapshot, String> {
    let active_session_root = state.sessions.active_root().map_err(map_session_err)?;
    let active_root = project_root_for_session(
        active_session_root.as_deref(),
        active_session_root
            .is_none()
            .then(|| resolve_workspace_root(state).ok())
            .flatten(),
    );

    let mut roots = load_known_workspaces();
    if let Some(active) = active_root {
        if !roots.iter().any(|root| root == &active) {
            roots.insert(0, active);
        }
    }

    let spaces = state
        .space_state
        .lock()
        .map_err(|_| "Space state lock poisoned.".to_string())?
        .clone()
        .normalized();
    let active_space_id = spaces.active_space_id.clone();
    let views = spaces
        .spaces
        .iter()
        .map(|space| SpaceView {
            id: space.id.clone(),
            name: space.name.clone(),
            emoji: space.emoji.clone(),
            is_default: space.id == DEFAULT_SPACE_ID,
            project_count: roots
                .iter()
                .filter(|root| spaces.space_for_project(&display_path(root)) == space.id)
                .count(),
        })
        .collect();
    let last_workspace_path = spaces
        .last_workspace_by_space_id
        .get(&active_space_id)
        .and_then(|path| require_known_workspace(Path::new(path)).ok())
        .map(|path| display_path(&path));

    Ok(SpacesSnapshot {
        active_space_id,
        spaces: views,
        last_workspace_path,
    })
}

fn remember_active_space_workspace(state: &AppState, root: &Path) -> Result<(), String> {
    let root = require_known_workspace(root)?;
    let key = display_path(&root);
    let mut guard = state
        .space_state
        .lock()
        .map_err(|_| "Space state lock poisoned.".to_string())?;
    let previous = guard.clone();
    guard.remember_active_workspace(&key);
    if let Err(error) = save_space_state(&guard) {
        *guard = previous;
        return Err(error);
    }
    Ok(())
}

fn adopt_workspace_into_active_space(state: &AppState, root: &Path) -> Result<(), String> {
    let root = require_known_workspace(root)?;
    let key = display_path(&root);
    let mut guard = state
        .space_state
        .lock()
        .map_err(|_| "Space state lock poisoned.".to_string())?;
    let previous = guard.clone();
    let active = guard.active_space_id.clone();
    if active != DEFAULT_SPACE_ID {
        guard.set_project_space(&key, &active)?;
    }
    guard.remember_active_workspace(&key);
    if let Err(error) = save_space_state(&guard) {
        *guard = previous;
        return Err(error);
    }
    Ok(())
}

fn restore_space_state(state: &AppState, previous: SpaceState) {
    if let Ok(mut guard) = state.space_state.lock() {
        *guard = previous;
        let _ = save_space_state(&guard);
    }
}

/// Pick the project folder, rejecting any candidate Zest cannot write to.
///
/// Order matters: an explicit launch directory or folder choice beats a
/// remembered one, and Documents/Zest catches the case where none of those are
/// usable. Every step is writability-checked, because a candidate that fails
/// only surfaces later as a storage error deep inside session startup, far from
/// the folder that caused it.
fn resolve_workspace_root(state: &AppState) -> Result<PathBuf, String> {
    if let Ok(guard) = state.workspace_root.lock() {
        if let Some(root) = guard.as_ref() {
            return Ok(root.clone());
        }
    }
    let resolved = initial_workspace_root().ok_or_else(no_writable_workspace_error)?;
    if let Ok(mut guard) = state.workspace_root.lock() {
        *guard = Some(resolved.clone());
    }
    Ok(resolved)
}

/// Storage for conversations that are intentionally not attached to a
/// project. This lives in Zest's user data area, never in the known-workspaces
/// list, so a free chat cannot accidentally become a project in the sidebar.
fn free_chats_root() -> Result<PathBuf, String> {
    let root = zest_config_dir()?.join("free-chats");
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    canonicalize_dir(root)
}

fn is_free_chat_root(root: &Path) -> bool {
    free_chats_root()
        .ok()
        .is_some_and(|free| display_path(&free) == display_path(root))
}

/// Keep a free chat detached from the remembered project list. The workspace
/// root remains available for the next project chat, but it must not be treated
/// as active while the user is in the project-less chat bucket.
fn project_root_for_session(
    active_session_root: Option<&Path>,
    fallback_root: Option<PathBuf>,
) -> Option<PathBuf> {
    match active_session_root {
        Some(root) if is_free_chat_root(root) => None,
        Some(root) => Some(root.to_path_buf()),
        None => fallback_root,
    }
}

fn set_workspace_root(state: &AppState, root: PathBuf) -> Result<PathBuf, String> {
    let root = canonicalize_dir(root)?;
    // Refuse here rather than at first write: the folder picker is the one
    // moment the user is already thinking about which folder to use.
    if is_install_dir(&root) {
        return Err(format!(
            "{WORKSPACE_NOT_WRITABLE}: {} is the Zest program folder, not a project. \
             Choose a folder inside your user account instead.",
            display_path(&root)
        ));
    }
    if !dir_is_writable(&root) {
        return Err(format!(
            "{WORKSPACE_NOT_WRITABLE}: Zest cannot save anything in {}. \
             Choose a folder you own, such as one under Documents.",
            display_path(&root)
        ));
    }
    persist_workspace(&root)?;
    if let Ok(mut guard) = state.workspace_root.lock() {
        *guard = Some(root.clone());
    }
    Ok(root)
}

fn open_store(root: &std::path::Path) -> Result<ThreadStore, String> {
    ThreadStore::open(root).map_err(|e| workspace_write_error(root, e))
}

/// Load the transcript together with its lifecycle projection, then close any
/// turn the current provider cannot resume after a process restart. The thread
/// snapshot returned here is the one the session should render and restore into
/// the agent; the warning keeps the recovery visible without exposing storage
/// details to the UI.
fn recover_chat_on_load(
    root: &std::path::Path,
    store: &ThreadStore,
    thread_id: &str,
    warning_already_present: bool,
) -> Result<(Thread, Option<String>, Option<RecoverableRun>), String> {
    let persistence = ChatPersistence::open(root).map_err(|error| error.to_string())?;
    let reconstructed = persistence
        .reconstruct_chat(store, thread_id)
        .map_err(|error| error.to_string())?;
    let recoverable_run = reconstructed.recoverable_run.clone();
    let had_unfinished_state =
        reconstructed.active_run.is_some() || !reconstructed.pending_interrupts.is_empty();
    let reconciliation = persistence
        .reconcile_after_restart(thread_id)
        .map_err(|error| error.to_string())?;
    let recovery_warning = if !(warning_already_present || reconstructed.thread_warning.is_some())
        && (had_unfinished_state
            || reconciliation.aborted_runs > 0
            || reconciliation.cancelled_interrupts > 0)
    {
        Some(
            "A previous turn was interrupted and closed safely. Its message is ready to resend."
                .into(),
        )
    } else {
        None
    };

    Ok((reconstructed.thread, recovery_warning, recoverable_run))
}

fn ensure_persist(state: &AppState, root: &std::path::Path) -> Result<PersistWorker, String> {
    let mut guard = state
        .persist
        .lock()
        .map_err(|_| "persist lock poisoned".to_string())?;
    let key = root.to_path_buf();
    if let Some(worker) = guard.get(&key) {
        return Ok(worker.clone());
    }
    let worker = PersistWorker::spawn(root).map_err(|e| e.to_string())?;
    guard.insert(key, worker.clone());
    Ok(worker)
}

struct ResolvedThread {
    thread: Thread,
    warning: Option<String>,
    created: bool,
    /// This thread has no row on disk yet and must not get one here.
    ///
    /// Distinct from `!created`: a loaded thread is also "not created", but it
    /// already exists and may be written to freely. A draft must survive as far
    /// as the first turn without ever being saved, or it shows up in history as
    /// a chat the user never started.
    draft: bool,
}

fn resolve_thread(
    root: &std::path::Path,
    store: &ThreadStore,
    provider_id: &str,
    config: &Config,
    allow_unowned: bool,
) -> Result<ResolvedThread, String> {
    let state = ProjectSessionState::load(root, provider_id);
    if let Some(id) = state.get(provider_id).thread_id {
        match store.load_for_provider(&id, provider_id) {
            Ok(loaded) => {
                if loaded.thread.provider_id.is_none() && !allow_unowned {
                    return Err(thread_provider_unknown_error(config, &loaded.thread.id));
                }
                let mut thread = loaded.thread;
                // Pin missing owner once; never rewrite a different owner.
                thread
                    .ensure_provider(provider_id)
                    .map_err(|e| e.to_string())?;
                return Ok(ResolvedThread {
                    thread,
                    warning: loaded.warning,
                    created: false,
                    draft: false,
                });
            }
            Err(ThreadLoadError::Corrupt { .. }) => {
                let thread = store
                    .create_for_provider(provider_id)
                    .map_err(|e| e.to_string())?;
                return Ok(ResolvedThread {
                    thread,
                    warning: Some(
                        "Chat history could not be restored, so a new conversation was started."
                            .into(),
                    ),
                    created: true,
                    draft: false,
                });
            }
            Err(ThreadLoadError::Missing(_)) => {
                // The pointer names a chat with no row on disk. That is not a
                // lost chat needing repair — it is the unsaved draft that
                // `delete_thread` leaves behind, which deliberately has no row
                // until the user sends something.
                //
                // Creating a replacement here is what produced the duplicate:
                // the draft kept its pointer, so every plain open of the
                // project (reopening it, switching Space, relaunching) minted
                // another persisted "Untitled chat" that the user never
                // started, alongside whichever chat they did start. Hand the
                // same draft back instead, under the id the pointer already
                // names, so a later send saves it exactly where the pointer
                // expects.
                let mut draft = Thread::new().with_provider(provider_id);
                draft.id = id.clone();
                return Ok(ResolvedThread {
                    thread: draft,
                    warning: None,
                    created: false,
                    draft: true,
                });
            }
            Err(ThreadLoadError::ProviderMismatch { .. })
            | Err(ThreadLoadError::UnsupportedVersion { .. }) => {
                // Fall through to a fresh provider-owned thread.
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    let thread = store
        .create_for_provider(provider_id)
        .map_err(|e| e.to_string())?;
    Ok(ResolvedThread {
        thread,
        warning: None,
        created: true,
        draft: false,
    })
}

/// Whether opening a thread should write it back to the store.
///
/// Opening stamps the branch and HEAD onto the thread, and for anything already
/// on disk that is worth saving. An unsaved draft is the exception and vetoes
/// the write outright: the stamp alone would create the history row the draft
/// exists to avoid — the phantom "Untitled chat" that appeared beside the chat
/// the user actually started. Its git context still lives in memory, and the
/// first turn writes it along with the message.
///
/// A function rather than an inline condition so the rule can be tested for
/// real instead of a test re-implementing it and drifting.
fn should_persist_on_open(
    claiming_legacy_thread: bool,
    metadata_changed: bool,
    is_unsaved_draft: bool,
) -> bool {
    (claiming_legacy_thread || metadata_changed) && !is_unsaved_draft
}

fn persist_provider_thread(
    root: &std::path::Path,
    provider_id: &str,
    thread_id: &str,
) -> Result<(), String> {
    let mut state = ProjectSessionState::load(root, provider_id);
    state.set_thread(provider_id, thread_id);
    state.save(root).map_err(|e| e.to_string())
}

fn persist_provider_model_effort(
    root: &std::path::Path,
    provider_id: &str,
    model: &str,
    effort: &str,
) -> Result<(), String> {
    let mut state = ProjectSessionState::load(root, provider_id);
    state.set_model_effort(provider_id, model, effort);
    state.save(root).map_err(|e| e.to_string())
}

fn session_capabilities(session: &Session) -> (String, Vec<ModelCapability>) {
    let descriptor = session.agent.descriptor();
    (
        descriptor.default_model,
        descriptor
            .models
            .into_iter()
            .map(|m| ModelCapability {
                id: m.id,
                efforts: m.efforts,
                context_window: m.context_window,
                supports_tools: m.supports_tools,
                supports_vision: m.supports_vision,
            })
            .collect(),
    )
}

/// Everything about a session except the conversation.
///
/// Changing a model or an effort level does not change the transcript, but the
/// only reply shape available carried every message anyway — so picking a model
/// serialized the whole conversation, sent it across the IPC boundary, and was
/// then discarded by a front end that already had it. The UI's own merge did
/// `{ ...info, messages: prev.messages }`, which says plainly that the
/// expensive half of the payload was never wanted.
fn session_meta_from(session: &Session, warning: Option<String>) -> SessionMeta {
    let (default_model, models) = session_capabilities(session);
    SessionMeta {
        session_id: session.session_id.clone(),
        provider: session.provider_id.clone(),
        label: session.provider_label.clone(),
        model: session.model.clone(),
        effort: session.effort.clone(),
        root: display_path(&session.root),
        is_free_chat: is_free_chat_root(&session.root),
        thread_id: session.thread_id.clone(),
        default_model,
        models,
        checkpoints: session
            .thread
            .checkpoints
            .clone()
            .into_iter()
            .map(ThreadCheckpointView::from)
            .collect(),
        warning,
        recovery: session.recovery.as_ref().map(TurnRecoveryView::from),
    }
}

fn session_info_from(session: &Session, warning: Option<String>) -> SessionInfo {
    let (default_model, models) = session_capabilities(session);
    SessionInfo {
        session_id: session.session_id.clone(),
        provider: session.provider_id.clone(),
        label: session.provider_label.clone(),
        model: session.model.clone(),
        effort: session.effort.clone(),
        root: display_path(&session.root),
        is_free_chat: is_free_chat_root(&session.root),
        thread_id: session.thread_id.clone(),
        default_model,
        models,
        checkpoints: session
            .thread
            .checkpoints
            .clone()
            .into_iter()
            .map(ThreadCheckpointView::from)
            .collect(),
        messages: session.thread.messages.clone(),
        warning,
        recovery: session.recovery.as_ref().map(TurnRecoveryView::from),
    }
}

fn apply_event_to_thread(thread: &mut Thread, event: &ChatEvent) {
    match event {
        ChatEvent::User {
            message_id, text, ..
        } => thread.apply_user(message_id, text),
        ChatEvent::AssistantStart {
            message_id,
            command,
            ..
        } => {
            thread.apply_assistant_start(message_id, command.as_deref());
        }
        ChatEvent::TextDelta {
            message_id, text, ..
        } => thread.apply_text_delta(message_id, text),
        ChatEvent::ThinkingDelta {
            message_id, text, ..
        } => thread.apply_thinking_delta(message_id, text),
        ChatEvent::ProviderActivity { .. } => {}
        ChatEvent::ToolCallStart {
            message_id,
            name,
            id,
            ..
        } => thread.apply_tool_start(message_id, id, name),
        ChatEvent::ToolCallUpdate { .. } => {}
        ChatEvent::ToolCallResult {
            message_id,
            name,
            id,
            summary,
            is_error,
            path,
            diff,
            metadata,
            ..
        } => {
            let core_meta = metadata.clone().map(|m| match m {
                ToolMetaView::Delegation {
                    provider_id,
                    model,
                    diff,
                    job_id,
                    stage,
                    attempt,
                    review_status,
                } => ToolMetadata::Delegation {
                    provider_id,
                    model,
                    diff,
                    usage: None,
                    job_id,
                    stage,
                    attempt,
                    review_status,
                },
            });
            thread.apply_tool_result(
                message_id,
                id,
                name,
                summary,
                *is_error,
                path.as_deref(),
                diff.as_deref(),
                core_meta,
            );
        }
        ChatEvent::ApprovalNeeded {
            message_id,
            approval_id,
            tool_name,
            tool_call_id,
            path,
            summary,
            diff,
            ..
        } => thread.apply_approval_needed(
            message_id,
            tool_call_id,
            tool_name,
            approval_id,
            path,
            summary,
            diff,
        ),
        ChatEvent::QuestionNeeded { .. } => {}
        ChatEvent::WorkspaceChanged { .. } => {}
        ChatEvent::Done { message_id, .. } => thread.apply_done(message_id),
        ChatEvent::Error {
            message_id,
            message,
            ..
        } => thread.apply_error(message_id, message),
        ChatEvent::Cancelled { message_id, .. } => {
            thread.apply_error(message_id, "turn cancelled");
        }
        ChatEvent::Warning { .. } => {}
    }
}

fn event_priority(event: &ChatEvent) -> PersistPriority {
    match event {
        ChatEvent::TextDelta { .. } | ChatEvent::ThinkingDelta { .. } => PersistPriority::Delta,
        _ => PersistPriority::Immediate,
    }
}

#[tauri::command]
async fn start_session(
    state: State<'_, AppState>,
    id: String,
    model: Option<String>,
    effort: Option<String>,
) -> Result<SessionInfo, String> {
    start_session_inner(state, id, model, effort, None, None).await
}

async fn start_session_inner(
    state: State<'_, AppState>,
    id: String,
    model: Option<String>,
    effort: Option<String>,
    root_override: Option<PathBuf>,
    // `thread_override` is `(thread, load warning, is an unsaved draft)`. The
    // draft flag has to travel with the thread: `open_project_chat` resolves
    // its own target and hands it over here, so a flag left behind at the
    // resolve site is a flag this function cannot see.
    thread_override: Option<(Thread, Option<String>, bool)>,
) -> Result<SessionInfo, String> {
    zest_core::load_env();

    let root = root_override.unwrap_or(resolve_workspace_root(&state)?);
    let config = if is_free_chat_root(&root) {
        config_for_free_chat(&state, &root)?
    } else {
        config_for_session(&state, &root)?
    };

    let (selectable, provider_label) = if config.providers.contains_key(&id) {
        let provider = ProviderRegistry::from_config(&config)
            .0
            .get(&id)
            .ok_or_else(|| format!("unknown provider `{id}`"))?;
        (
            provider.auth_status().selectable(),
            configured_provider_view(&id, &config).label,
        )
    } else {
        let slot = detect_all()
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| format!("unknown provider `{id}`"))?;
        (slot.status.selectable(), slot.label.to_string())
    };

    if !selectable {
        return Err(format!(
            "{provider_label} is not ready — configure it first"
        ));
    }

    // Only the local half. Opening a chat waits for the gateway's port, which is
    // cheap, and *not* for a live turn against the account, which is a network
    // round trip that costs tokens — that used to make every launch sit on
    // "Opening your session…" until the model answered. The caller verifies the
    // account in the background and surfaces a banner if it turns out to be
    // unusable, so a cooled-down session is reported rather than waited for.
    if local_gateway_url(&config, &id).is_some() {
        ensure_gateway_ready(&state.gateway, &config, &id)
            .await
            .map_err(|failure| failure.user_message())?;
    }

    let prefs = ProjectSessionState::load(&root, &id).get(&id);

    // Only what the caller explicitly asked for is `explicit`. The sticky
    // values go in as *remembered*, which RuntimeBuilder drops instead of
    // erroring when they do not fit this provider — otherwise one stale entry
    // makes the provider impossible to select and therefore impossible to fix.
    let explicit_model = model.filter(|m| !m.trim().is_empty());
    let explicit_effort = effort
        .filter(|e| !e.trim().is_empty())
        .map(|e| normalize_effort(&e));

    let store = open_store(&root)?;
    let (mut thread, load_warning, thread_created, thread_is_draft) = match thread_override {
        Some((thread, warning, draft)) => (thread, warning, false, draft),
        None => {
            let resolved = resolve_thread(&root, &store, &id, &config, false)?;
            (
                resolved.thread,
                resolved.warning,
                resolved.created,
                resolved.draft,
            )
        }
    };
    // Opening a chat while its existing turn is still live must not run the
    // restart reconciler: that reconciler quite correctly closes unfinished
    // runs after a process crash, but this is an in-process navigation.
    let live_turn = state
        .sessions
        .active_turn_for_thread(&thread.id)
        .map_err(map_session_err)?
        .is_some();
    let claiming_legacy_thread = thread.provider_id.is_none();
    let (recovery_warning, recovery) = if live_turn {
        (None, None)
    } else {
        match recover_chat_on_load(&root, &store, &thread.id, load_warning.is_some()) {
            Ok((recovered_thread, warning, recovery)) => {
                thread = recovered_thread;
                (warning, recovery)
            }
            Err(_) => (
                Some(
                    "Chat recovery state could not be checked; the saved transcript is still available."
                        .into(),
                ),
                None,
            ),
        }
    };
    thread.ensure_provider(&id).map_err(|e| e.to_string())?;
    let initial_branch = read_git_branch(&root);
    let mut thread_metadata_changed =
        thread.ensure_git_context(initial_branch.clone(), read_git_head(&root).await);
    thread_metadata_changed |= thread.record_git_branch(initial_branch);

    let approval_hub = Arc::new(ApprovalHub::new());
    let question_hub = Arc::new(QuestionHub::new());
    let approver: Arc<dyn Approver> = Arc::new(HubApprover {
        hub: approval_hub.clone(),
    });
    let questioner: Arc<dyn Questioner> = Arc::new(HubQuestioner {
        hub: question_hub.clone(),
    });
    let session_config = config.clone();

    let mut builder = RuntimeBuilder::new(&root)
        .with_config(config)
        .with_provider(&id)
        .with_system(DEFAULT_SYSTEM)
        .with_approver(approver)
        .with_questioner(questioner)
        .with_policy(state.policy.clone())
        .with_browser_adapter(state.browser.adapter())
        .with_parent_thread_id(&thread.id)
        .with_remembered_options(prefs.model, prefs.effort)
        .enable_external_agents(true)
        .register_write_tools(true)
        // Every non-allowlisted command reaches HubApprover, which is the same
        // card `write_file` already uses.
        .register_exec_tools(true);
    if let Some(model) = explicit_model {
        builder = builder.with_model(model);
    }
    if let Some(effort) = explicit_effort {
        builder = builder.with_effort(effort);
    }

    let runtime = match builder.build() {
        Ok(runtime) => runtime,
        Err(error) => {
            if thread_created {
                let _ = store.delete(&thread.id);
            }
            return Err(error.to_string());
        }
    };
    let runtime_warnings = runtime.warnings.clone();
    let mut agent = runtime.agent;
    agent.messages = thread.agent_messages.clone();
    agent.provider_session = thread.provider_session.clone();
    agent.provider_interaction = Some(Arc::new(DesktopProviderInteraction {
        approval_hub: approval_hub.clone(),
        question_hub: question_hub.clone(),
    }));

    // A legacy thread is claimed only after the target provider has built a
    // usable runtime. Git context is persisted at the same point so a failed
    // provider switch cannot leave half-open chat metadata behind.
    if should_persist_on_open(
        claiming_legacy_thread,
        thread_metadata_changed,
        thread_is_draft,
    ) {
        if let Err(error) = store.save(&thread) {
            if thread_created {
                let _ = store.delete(&thread.id);
            }
            return Err(error.to_string());
        }
    }

    if let Err(error) = persist_provider_model_effort(&root, &id, &runtime.model, &runtime.effort) {
        if thread_created {
            let _ = store.delete(&thread.id);
        }
        return Err(error);
    }
    if let Err(error) = persist_provider_thread(&root, &id, &thread.id) {
        if thread_created {
            let _ = store.delete(&thread.id);
        }
        return Err(error);
    }
    // Only remember a provider after its runtime and thread have been built.
    // A failed provider switch must not poison the next launch with a provider
    // that never became a live session.
    if let Err(error) = persist_choice(&id) {
        if thread_created {
            let _ = store.delete(&thread.id);
        }
        return Err(error);
    }

    let thread_id = thread.id.clone();
    let session = Session {
        session_id: String::new(),
        agent,
        model: runtime.model,
        effort: runtime.effort,
        provider_id: id,
        provider_label,
        root,
        thread_id: thread_id.clone(),
        thread,
        recovery,
        base_system: runtime.base_system,
        skills: runtime.skills,
        approval_hub,
        question_hub,
    };

    let session_is_free_chat = is_free_chat_root(&session.root);
    if let Err(error) = state.sessions.set_session(session) {
        if thread_created {
            let _ = store.delete(&thread_id);
        }
        return Err(map_session_err(error));
    }
    if !session_is_free_chat {
        remember_workspace_config(&state, &session_config);
    }

    // A dropped preference is worth saying out loud — otherwise the picker just
    // shows a different model than last time with no explanation.
    let warning = merge_warnings(load_warning, runtime_warnings);
    let warning = merge_warnings(warning, recovery_warning.into_iter().collect());

    let info = state
        .sessions
        .session_info_snapshot(|s| session_info_from(s, warning.clone()))
        .map_err(map_session_err)?
        .ok_or_else(|| map_session_err(SessionError::NoSession))?;
    Ok(info)
}

/// Join a thread-load warning with any runtime warnings into the single slot
/// `SessionInfo` has for them.
fn merge_warnings(load_warning: Option<String>, runtime: Vec<String>) -> Option<String> {
    let mut all: Vec<String> = load_warning.into_iter().collect();
    all.extend(runtime);
    (!all.is_empty()).then(|| all.join("; "))
}

#[tauri::command]
fn update_session_options(
    state: State<'_, AppState>,
    model: Option<String>,
    effort: Option<String>,
) -> Result<SessionMeta, String> {
    state.sessions.require_idle().map_err(map_session_err)?;
    state
        .sessions
        .with_session_mut(|session| -> Result<SessionMeta, String> {
            let next_model = model
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| session.model.clone());
            let next_effort = effort
                .filter(|e| !e.trim().is_empty())
                .map(|e| normalize_effort(&e))
                .unwrap_or_else(|| session.effort.clone());
            session.agent.validate_options(&next_model, &next_effort)?;
            session.model = next_model.clone();
            session.agent.model = next_model;
            session.effort = next_effort.clone();
            session.agent.effort = next_effort;
            persist_provider_model_effort(
                &session.root,
                &session.provider_id,
                &session.model,
                &session.effort,
            )?;
            Ok(session_meta_from(session, None))
        })
        .map_err(map_session_err)
        .and_then(|r| r)
}

/// Atomically reset sticky model+effort for the active provider (clears prefs).
#[tauri::command]
fn reset_session_options(state: State<'_, AppState>) -> Result<SessionMeta, String> {
    state.sessions.require_idle().map_err(map_session_err)?;
    state
        .sessions
        .with_session_mut(|session| -> Result<SessionMeta, String> {
            let descriptor = session.agent.descriptor();
            let next_model = descriptor.default_model.clone();
            let next_effort = "high".to_string();
            session.agent.validate_options(&next_model, &next_effort)?;
            session.model = next_model.clone();
            session.agent.model = next_model;
            session.effort = next_effort.clone();
            session.agent.effort = next_effort;
            let mut prefs = ProjectSessionState::load(&session.root, &session.provider_id);
            prefs.clear_model_effort(&session.provider_id);
            prefs.save(&session.root).map_err(|e| e.to_string())?;
            Ok(session_meta_from(session, None))
        })
        .map_err(map_session_err)
        .and_then(|r| r)
}

#[tauri::command]
fn list_threads(state: State<'_, AppState>) -> Result<Vec<ThreadSummary>, String> {
    state
        .sessions
        .with_session_mut(|session| {
            open_store(&session.root)?
                .list_for_provider(&session.provider_id)
                .map_err(|e| e.to_string())
        })
        .map_err(map_session_err)
        .and_then(|r| r)
}

#[tauri::command]
fn list_spaces(state: State<'_, AppState>) -> Result<SpacesSnapshot, String> {
    space_snapshot(&state)
}

#[tauri::command]
fn set_active_space(
    state: State<'_, AppState>,
    space_id: String,
    current_workspace_path: Option<String>,
) -> Result<SpacesSnapshot, String> {
    let current_workspace = current_workspace_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(|path| require_known_workspace(Path::new(path)))
        .transpose()?;
    let mut guard = state
        .space_state
        .lock()
        .map_err(|_| "Space state lock poisoned.".to_string())?;
    let previous = guard.clone();
    let space_id = space_id.trim();
    if guard.space(space_id).is_none() {
        return Err("That Space no longer exists.".to_string());
    }

    let previous_active = guard.active_space_id.clone();
    if let Some(root) = current_workspace {
        let key = display_path(&root);
        if guard.space_for_project(&key) == previous_active {
            guard
                .last_workspace_by_space_id
                .insert(previous_active, key);
        }
    }
    guard.active_space_id = space_id.to_string();
    if let Err(error) = save_space_state(&guard) {
        *guard = previous;
        return Err(error);
    }
    drop(guard);
    space_snapshot(&state)
}

#[tauri::command]
fn create_space(
    state: State<'_, AppState>,
    name: String,
    emoji: Option<String>,
) -> Result<SpacesSnapshot, String> {
    let mut guard = state
        .space_state
        .lock()
        .map_err(|_| "Space state lock poisoned.".to_string())?;
    let previous = guard.clone();
    guard.create_space(new_id("space"), &name, emoji)?;
    if let Err(error) = save_space_state(&guard) {
        *guard = previous;
        return Err(error);
    }
    drop(guard);
    space_snapshot(&state)
}

#[tauri::command]
fn update_space(
    state: State<'_, AppState>,
    space_id: String,
    name: String,
    emoji: Option<String>,
) -> Result<SpacesSnapshot, String> {
    let mut guard = state
        .space_state
        .lock()
        .map_err(|_| "Space state lock poisoned.".to_string())?;
    let previous = guard.clone();
    guard.update_space(space_id.trim(), &name, emoji)?;
    if let Err(error) = save_space_state(&guard) {
        *guard = previous;
        return Err(error);
    }
    drop(guard);
    space_snapshot(&state)
}

#[tauri::command]
fn delete_space(state: State<'_, AppState>, space_id: String) -> Result<SpacesSnapshot, String> {
    let mut guard = state
        .space_state
        .lock()
        .map_err(|_| "Space state lock poisoned.".to_string())?;
    let previous = guard.clone();
    guard.delete_space(space_id.trim())?;
    if let Err(error) = save_space_state(&guard) {
        *guard = previous;
        return Err(error);
    }
    drop(guard);
    space_snapshot(&state)
}

#[tauri::command]
fn move_project_to_space(
    state: State<'_, AppState>,
    project_path: String,
    space_id: String,
) -> Result<SpacesSnapshot, String> {
    let root = require_known_workspace(Path::new(project_path.trim()))?;
    let key = display_path(&root);
    let mut guard = state
        .space_state
        .lock()
        .map_err(|_| "Space state lock poisoned.".to_string())?;
    let previous = guard.clone();
    guard.set_project_space(&key, space_id.trim())?;
    if guard.active_space_id == space_id.trim() {
        guard.remember_active_workspace(&key);
    }
    if let Err(error) = save_space_state(&guard) {
        *guard = previous;
        return Err(error);
    }
    drop(guard);
    space_snapshot(&state)
}

/// Remove a project from Zest's recent workspace registry without touching its
/// folder or the chats stored inside it.
#[tauri::command]
fn forget_workspace(
    state: State<'_, AppState>,
    project_path: String,
) -> Result<SpacesSnapshot, String> {
    let (_root, root_key, known_roots) = workspace_removal_target(&state, &project_path)?;
    remove_known_workspace_metadata(&state, &root_key, &known_roots)?;
    space_snapshot(&state)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectChats {
    name: String,
    /// `None` is the user-local free-chat bucket rendered under RECENT.
    path: Option<String>,
    active: bool,
    space_id: String,
    threads: Vec<ThreadSummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SpaceView {
    id: String,
    name: String,
    emoji: Option<String>,
    is_default: bool,
    project_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SpacesSnapshot {
    active_space_id: String,
    spaces: Vec<SpaceView>,
    last_workspace_path: Option<String>,
}

fn project_display_name(root: &Path) -> String {
    root.file_name()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| display_path(root))
}

/// Return thread summaries without reparsing unchanged conversation files.
///
/// The thread format intentionally keeps the full UI projection and provider
/// wire history together, which makes a full JSON parse needlessly expensive
/// for a sidebar that only needs six metadata fields. Keep the cache in the
/// desktop process and use file metadata as a cheap cross-process invalidation
/// signal. A changed, new, corrupt, or removed file is handled exactly like the
/// uncached scanner below: it is reparsed or skipped rather than making the
/// sidebar fail.
fn list_cached_threads(
    store: &ThreadStore,
    provider_id: Option<&str>,
    cache: &mut ProjectSummaryCache,
) -> Vec<ThreadSummary> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let Ok(entries) = std::fs::read_dir(store.dir()) else {
        cache.files.clear();
        return out;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".json") || name.contains(".corrupt") {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let modified = meta.modified().ok();
        let length = meta.len();
        seen.insert(name.to_string());

        let summary = cache
            .files
            .get(name)
            .filter(|cached| cached.modified == modified && cached.length == length)
            .map(|cached| cached.summary.clone())
            .or_else(|| {
                let body = std::fs::read_to_string(&path).ok()?;
                let thread = serde_json::from_str::<Thread>(&body).ok()?;
                if thread.version > THREAD_FORMAT_VERSION {
                    return None;
                }
                let summary = thread.summary();
                cache.files.insert(
                    name.to_string(),
                    CachedThreadSummary {
                        modified,
                        length,
                        summary: summary.clone(),
                    },
                );
                Some(summary)
            });

        if let Some(summary) = summary {
            if let Some(wanted) = provider_id {
                if summary.provider_id.as_deref() != Some(wanted) {
                    continue;
                }
            }
            out.push(summary);
        } else {
            // Do not retain an old summary after a file becomes corrupt or
            // unsupported; a later repair must be allowed to repopulate it.
            cache.files.remove(name);
        }
    }

    cache.files.retain(|name, _| seen.contains(name));
    out.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });
    out
}

/// Chats grouped by known project folders (MRU), plus the free-chat bucket for
/// RECENT, for the sidebar.
#[tauri::command]
fn list_chat_projects(state: State<'_, AppState>) -> Result<Vec<ProjectChats>, String> {
    let active_session_root = state.sessions.active_root().map_err(map_session_err)?;
    let active_root = project_root_for_session(
        active_session_root.as_deref(),
        active_session_root
            .is_none()
            .then(|| resolve_workspace_root(&state).ok())
            .flatten(),
    );
    if let Some(active_root) = active_root.as_ref() {
        remember_workspace(active_root);
    }

    let spaces = state
        .space_state
        .lock()
        .map_err(|_| "Space state lock poisoned.".to_string())?
        .clone()
        .normalized();
    let active_space_id = spaces.active_space_id.clone();

    let mut roots = load_known_workspaces();
    if let Some(active_root) = active_root.as_ref() {
        if !roots.iter().any(|p| p == active_root) {
            roots.insert(0, active_root.clone());
        }
    }

    let free_root = free_chats_root()?;
    let mut cache_roots: HashSet<PathBuf> = roots.iter().cloned().collect();
    cache_roots.insert(free_root.clone());
    let mut cache = state
        .chat_summary_cache
        .lock()
        .map_err(|_| "chat summary cache lock poisoned".to_string())?;
    let mut out = Vec::new();
    for root in roots {
        if !root.is_dir() {
            continue;
        }
        let space_id = spaces.space_for_project(&display_path(&root)).to_string();
        if space_id != active_space_id {
            continue;
        }
        let threads = match open_store(&root) {
            Ok(store) => list_cached_threads(
                &store,
                None,
                cache.projects.entry(root.clone()).or_default(),
            ),
            Err(_) => Vec::new(),
        };
        let active = active_root.as_ref().is_some_and(|active| root == *active);
        out.push(ProjectChats {
            name: project_display_name(&root),
            path: Some(display_path(&root)),
            active,
            space_id,
            threads,
        });
    }

    let free_threads = match open_store(&free_root) {
        Ok(store) => list_cached_threads(
            &store,
            None,
            cache.projects.entry(free_root.clone()).or_default(),
        ),
        Err(_) => Vec::new(),
    };
    if !free_threads.is_empty() {
        out.push(ProjectChats {
            name: "Free chats".to_string(),
            path: None,
            active: active_session_root
                .as_ref()
                .is_some_and(|root| is_free_chat_root(root)),
            space_id: DEFAULT_SPACE_ID.to_string(),
            threads: free_threads,
        });
    }
    cache.projects.retain(|root, _| cache_roots.contains(root));

    // Active project first; then by newest thread activity.
    out.sort_by(|a, b| match (a.active, b.active) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => {
            let a_t = a.threads.first().map(|t| t.updated_at).unwrap_or(0);
            let b_t = b.threads.first().map(|t| t.updated_at).unwrap_or(0);
            b_t.cmp(&a_t).then_with(|| a.name.cmp(&b.name))
        }
    });
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversationProviderChoice {
    id: String,
    label: String,
    model: String,
}

fn configured_provider_choices(config: &Config) -> Vec<ConversationProviderChoice> {
    config
        .providers
        .keys()
        .filter_map(|id| {
            let view = configured_provider_view(id, config);
            view.selectable.then_some(ConversationProviderChoice {
                id: id.clone(),
                label: view.label,
                model: view.default_model,
            })
        })
        .collect()
}

fn provider_label_for_config(config: &Config, provider_id: &str) -> String {
    if config.providers.contains_key(provider_id) {
        configured_provider_view(provider_id, config).label
    } else {
        title_case_id(provider_id)
    }
}

fn thread_provider_unknown_error(config: &Config, thread_id: &str) -> String {
    desktop_err_with_details(
        "thread_provider_unknown",
        "This chat has no provider owner. Choose one before reopening it.",
        Some(serde_json::json!({
            "threadId": thread_id,
            "availableProviders": configured_provider_choices(config),
        })),
    )
}

fn provider_unavailable_error(
    config: &Config,
    provider_id: &str,
    thread_id: Option<&str>,
) -> String {
    let label = provider_label_for_config(config, provider_id);
    let configured = config.providers.contains_key(provider_id);
    let message = if configured {
        format!("{label} is not ready for this project.")
    } else {
        format!("{label} is not configured for this project.")
    };
    desktop_err_with_details(
        "provider_unavailable",
        message,
        Some(serde_json::json!({
            "threadId": thread_id,
            "providerId": provider_id,
            "providerLabel": label,
            "configured": configured,
            "availableProviders": configured_provider_choices(config),
        })),
    )
}

fn provider_is_selectable(config: &Config, provider_id: &str) -> bool {
    config.providers.contains_key(provider_id)
        && configured_provider_view(provider_id, config).selectable
}

/// Pick the provider that can serve a project chat without crossing a thread's
/// ownership boundary.
///
/// `thread_provider` distinguishes three cases: no thread was requested,
/// a saved thread has an owner, or a legacy saved thread has no owner. Only
/// the first case may use the project's default/only-provider convenience.
fn select_project_provider(
    config: &Config,
    requested_provider: &str,
    thread_provider: Option<Option<&str>>,
    explicit_provider: Option<&str>,
    thread_id: Option<&str>,
) -> Result<String, String> {
    if let Some(Some(owner)) = thread_provider {
        if let Some(chosen) = explicit_provider {
            if chosen != owner {
                return Err(desktop_err(
                    "thread_provider_mismatch",
                    format!(
                        "This chat belongs to {}, not {}. Open a copy to use another provider.",
                        provider_label_for_config(config, owner),
                        provider_label_for_config(config, chosen),
                    ),
                ));
            }
        }
        if config.providers.contains_key(owner) {
            return Ok(owner.to_string());
        }
        return Err(provider_unavailable_error(config, owner, thread_id));
    }

    if let Some(chosen) = explicit_provider {
        if config.providers.contains_key(chosen) {
            return Ok(chosen.to_string());
        }
        return Err(provider_unavailable_error(config, chosen, thread_id));
    }

    if matches!(thread_provider, Some(None)) {
        return Err(thread_provider_unknown_error(
            config,
            thread_id.unwrap_or("unknown"),
        ));
    }

    if config.providers.contains_key(requested_provider) {
        return Ok(requested_provider.to_string());
    }

    if let Some(default) = config.default_target().and_then(|target| {
        config
            .providers
            .contains_key(&target.provider)
            .then_some(target.provider)
    }) {
        return Ok(default.to_string());
    }

    if config.providers.len() == 1 {
        return config.providers.keys().next().cloned().ok_or_else(|| {
            desktop_err(
                "provider_unavailable",
                "This project has no provider configured.",
            )
        });
    }

    if config.providers.is_empty() {
        return Err(desktop_err(
            "provider_unavailable",
            "This project has no provider configured. Add one to zest.toml before opening a chat.",
        ));
    }

    Err(desktop_err(
        "provider_unavailable",
        "The selected provider is not configured for this project, and the project has no default provider.",
    ))
}

#[cfg(test)]
mod project_provider_tests {
    use super::*;

    #[test]
    fn keeps_requested_provider_when_project_declares_it() {
        let config = Config::parse(
            r#"
[providers.codex]
kind = "gateway"
base_url = "http://127.0.0.1:8317"
model = "gpt-5.6-terra"
"#,
        )
        .unwrap();

        assert_eq!(
            select_project_provider(&config, "codex", None, None, None).unwrap(),
            "codex"
        );
    }

    #[test]
    fn keeps_default_for_new_project_chats_when_requested_provider_is_missing() {
        let config = Config::parse(
            r#"
[providers.codex]
kind = "gateway"
base_url = "http://127.0.0.1:8317"
model = "gpt-5.6-terra"

[default]
provider = "codex"
"#,
        )
        .unwrap();

        assert_eq!(
            select_project_provider(&config, "deepseek", None, None, None).unwrap(),
            "codex"
        );
    }

    #[test]
    fn does_not_reopen_a_thread_with_a_different_provider() {
        let config = Config::parse(
            r#"
[providers.codex]
kind = "gateway"
base_url = "http://127.0.0.1:8317"
model = "gpt-5.6-terra"
"#,
        )
        .unwrap();

        let error = select_project_provider(
            &config,
            "codex",
            Some(Some("deepseek")),
            None,
            Some("thread-1"),
        )
        .expect_err("a thread owner is a hard boundary");
        assert!(error.contains("not configured for this project"));
    }

    #[test]
    fn legacy_thread_requires_an_explicit_owner() {
        let config = Config::parse(
            r#"
[providers.codex]
kind = "gateway"
base_url = "http://127.0.0.1:8317"
model = "gpt-5.6-terra"
"#,
        )
        .unwrap();

        let error =
            select_project_provider(&config, "codex", Some(None), None, Some("thread-legacy"))
                .expect_err("legacy threads must not inherit the current provider");
        assert!(error.contains("thread_provider_unknown"));

        assert_eq!(
            select_project_provider(
                &config,
                "codex",
                Some(None),
                Some("codex"),
                Some("thread-legacy"),
            )
            .unwrap(),
            "codex"
        );
    }
}

#[cfg(test)]
mod chat_summary_tests {
    use super::*;

    #[test]
    fn cached_summaries_follow_thread_changes_and_deletes() {
        let root = std::env::temp_dir().join(format!("zest-chat-cache-{}", new_id("test")));
        let store = ThreadStore::open(&root).unwrap();
        let mut first = Thread::new().with_provider("codex");
        let mut second = Thread::new().with_provider("codex");
        first.apply_user("user-1", "hello");
        store.save(&first).unwrap();
        store.save(&second).unwrap();

        let mut cache = ProjectSummaryCache::default();
        let initial = list_cached_threads(&store, Some("codex"), &mut cache);
        assert_eq!(initial.len(), 2);
        assert_eq!(cache.files.len(), 2);

        second.set_pinned(true);
        store.save(&second).unwrap();
        let pinned_first = list_cached_threads(&store, Some("codex"), &mut cache);
        assert_eq!(
            pinned_first.first().map(|summary| summary.id.as_str()),
            Some(second.id.as_str())
        );
        assert!(pinned_first.first().is_some_and(|summary| summary.pinned));

        let other_provider = Thread::new().with_provider("claude");
        store.save(&other_provider).unwrap();
        let all_providers = list_cached_threads(&store, None, &mut cache);
        assert_eq!(all_providers.len(), 3);
        assert!(all_providers
            .iter()
            .any(|summary| summary.id == other_provider.id));

        first.apply_user("user-2", "world");
        store.save(&first).unwrap();
        let changed = list_cached_threads(&store, Some("codex"), &mut cache);
        let changed_first = changed
            .iter()
            .find(|summary| summary.id == first.id)
            .unwrap();
        assert_eq!(changed_first.message_count, 2);

        store.delete(&second.id).unwrap();
        let remaining = list_cached_threads(&store, Some("codex"), &mut cache);
        assert_eq!(remaining.len(), 1);
        assert_eq!(cache.files.len(), 2);

        let _ = std::fs::remove_dir_all(root);
    }

    /// Deleting the open chat leaves an unsaved draft and points the project at
    /// it. Reopening the project then has to hand that same draft back — it
    /// used to mint a persisted replacement, so the sidebar grew an "Untitled
    /// chat" nobody started next to whichever chat the user did start.
    #[test]
    fn reopening_after_deleting_the_open_chat_adds_no_history_row() {
        let root = std::env::temp_dir().join(format!("zest-delete-reopen-{}", new_id("test")));
        let store = ThreadStore::open(&root).unwrap();
        let config = Config::default();

        let mut only = store.create_for_provider("codex").unwrap();
        only.apply_user("user-1", "hello");
        store.save(&only).unwrap();
        persist_provider_thread(&root, "codex", &only.id).unwrap();

        // What `delete_thread` does: drop the row, move the session onto an
        // unsaved draft, and point the project at that draft.
        store.delete(&only.id).unwrap();
        let draft = Thread::new().with_provider("codex");
        persist_provider_thread(&root, "codex", &draft.id).unwrap();
        assert_eq!(store.list().unwrap().len(), 0);

        // Any plain reopen of the project — relaunch, Space switch, project
        // click — lands here.
        let resolved = resolve_thread(&root, &store, "codex", &config, false).unwrap();
        assert!(!resolved.created, "reopening must not create a chat");
        assert_eq!(
            resolved.thread.id, draft.id,
            "the draft must keep its id so a later send saves where the pointer points"
        );
        assert_eq!(
            store.list().unwrap().len(),
            0,
            "an unsent draft must not appear in history"
        );

        // Reopening twice more must stay at zero rather than accumulate.
        let _ = resolve_thread(&root, &store, "codex", &config, false).unwrap();
        let _ = resolve_thread(&root, &store, "codex", &config, false).unwrap();
        assert_eq!(store.list().unwrap().len(), 0);

        let _ = std::fs::remove_dir_all(root);
    }

    /// Resolving the draft is only half of it. `start_session_inner` stamps the
    /// branch and head onto whatever thread it opens, and that metadata change
    /// used to trigger a save — quietly turning the draft into the history row
    /// it exists to avoid. Guard the flag that vetoes that write.
    #[test]
    fn an_unsaved_draft_is_not_written_by_a_metadata_stamp() {
        let root = std::env::temp_dir().join(format!("zest-draft-stamp-{}", new_id("test")));
        let store = ThreadStore::open(&root).unwrap();

        let only = store.create_for_provider("codex").unwrap();
        persist_provider_thread(&root, "codex", &only.id).unwrap();
        store.delete(&only.id).unwrap();
        let draft = Thread::new().with_provider("codex");
        persist_provider_thread(&root, "codex", &draft.id).unwrap();

        let resolved = resolve_thread(&root, &store, "codex", &Config::default(), false).unwrap();
        assert!(resolved.draft, "a pointer with no row resolves to a draft");

        // The stamp itself must still happen — the session needs the context —
        // it just must not reach disk.
        let mut thread = resolved.thread;
        let changed = thread.ensure_git_context(Some("dev".into()), Some("abc123".into()));
        assert!(changed, "stamping a fresh draft is a metadata change");
        assert!(
            !should_persist_on_open(false, changed, resolved.draft),
            "a metadata stamp must not persist an unsaved draft"
        );
        assert_eq!(
            store.list().unwrap().len(),
            0,
            "the draft must not have been written to history"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    /// The draft flag is only useful if it survives the handoff. It is dropped
    /// once already: `open_project_chat` resolves its own thread and passes it
    /// to `start_session_inner` as an override, and when that override could
    /// not carry the flag the draft was saved anyway — which is why deleting
    /// the open chat and creating a new one produced two.
    #[test]
    fn the_open_decision_covers_every_thread_a_session_can_start_from() {
        // Loaded from disk, git context moved on: save it.
        assert!(should_persist_on_open(false, true, false));
        // Freshly created by `create_for_provider`: the row exists, save it.
        assert!(should_persist_on_open(false, true, false));
        // A legacy thread being claimed: save it even without other changes.
        assert!(should_persist_on_open(true, false, false));
        // Nothing changed and nothing to claim: no write.
        assert!(!should_persist_on_open(false, false, false));
        // An unsaved draft: never, whatever else is true.
        assert!(!should_persist_on_open(false, true, true));
        assert!(!should_persist_on_open(true, true, true));
    }

    /// The other half: a project with no pointer at all still gets a chat.
    #[test]
    fn a_project_with_no_pointer_still_resolves_to_a_new_chat() {
        let root = std::env::temp_dir().join(format!("zest-fresh-project-{}", new_id("test")));
        let store = ThreadStore::open(&root).unwrap();
        let resolved = resolve_thread(&root, &store, "codex", &Config::default(), false).unwrap();
        assert!(resolved.created);
        assert_eq!(store.list().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod chat_recovery_tests {
    use super::*;
    use zest_core::RunStatus;

    #[test]
    fn stale_lifecycle_is_closed_and_reported_to_session_loader() {
        let root = std::env::temp_dir().join(format!("zest-chat-recovery-{}", new_id("test")));
        let store = ThreadStore::open(&root).unwrap();
        let mut thread = store.create_for_provider("codex").unwrap();
        thread.apply_user("user-restart", "Continue the interrupted work");
        store.save(&thread).unwrap();
        let persistence = ChatPersistence::open(&root).unwrap();
        let run = persistence
            .runs
            .create_or_resume_for_turn(
                "run-restart",
                &thread.id,
                "codex",
                "user-restart",
                "assistant-restart",
            )
            .unwrap();
        persistence
            .interrupts
            .create(
                "interrupt-restart",
                &run.run_id,
                &thread.id,
                json!({ "kind": "approval" }),
            )
            .unwrap();

        let (recovered, warning, retry) =
            recover_chat_on_load(&root, &store, &thread.id, false).unwrap();
        assert_eq!(recovered.id, thread.id);
        assert_eq!(
            warning.as_deref(),
            Some("A previous turn was interrupted and closed safely. Its message is ready to resend.")
        );
        assert_eq!(
            retry,
            Some(RecoverableRun {
                run_id: run.run_id.clone(),
                user_message_id: "user-restart".into(),
            })
        );
        assert_eq!(
            persistence.runs.load(&run.run_id).unwrap().unwrap().status,
            RunStatus::Aborted
        );
        assert!(persistence
            .interrupts
            .list_pending(&thread.id)
            .unwrap()
            .is_empty());

        let _ = std::fs::remove_dir_all(root);
    }
}

/// Switch project (and optional thread) while keeping the current provider.
/// A `null` root opens the user-local free-chat store without changing the
/// active workspace.
#[tauri::command]
async fn open_project_chat(
    state: State<'_, AppState>,
    root: Option<String>,
    thread_id: Option<String>,
    new_thread: Option<bool>,
    provider_id: Option<String>,
    copy_thread: Option<bool>,
) -> Result<SessionInfo, String> {
    let requested_provider = state
        .sessions
        .session_info_snapshot(|s| s.provider_id.clone())
        .map_err(map_session_err)?
        .or_else(last_provider)
        .ok_or_else(|| desktop_err("invalid", "no provider — connect one first"))?;

    // Validate the target before changing the active workspace. In particular,
    // a project-local zest.toml may intentionally omit the provider used by the
    // current chat.
    let free_chat = root.is_none();
    let previous_root = resolve_workspace_root(&state).ok();
    let root = match root {
        Some(raw) => canonicalize_dir(PathBuf::from(raw.trim()))?,
        None => free_chats_root()?,
    };
    let config = if free_chat {
        config_for_free_chat(&state, &root)?
    } else {
        config_for_session(&state, &root)?
    };
    let thread_id = thread_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    let explicit_provider = provider_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    let new_thread = new_thread.unwrap_or(false);
    let copy_thread = copy_thread.unwrap_or(false);
    let target_store = open_store(&root)?;
    let loaded_target = if !new_thread {
        thread_id
            .map(|tid| target_store.load_typed(tid).map_err(|e| e.to_string()))
            .transpose()?
    } else {
        None
    };
    let thread_provider = loaded_target
        .as_ref()
        .map(|loaded| loaded.thread.provider_id.as_deref());
    let copying_to_another_provider =
        copy_thread && explicit_provider.is_some() && loaded_target.is_some();
    let selection_thread_provider = if copying_to_another_provider {
        None
    } else {
        thread_provider
    };
    let provider_id = select_project_provider(
        &config,
        &requested_provider,
        selection_thread_provider,
        explicit_provider,
        loaded_target
            .as_ref()
            .map(|loaded| loaded.thread.id.as_str()),
    )?;

    // A saved chat must not fall through to the generic session-start error
    // when its owner is configured but currently unavailable. Return the same
    // recovery payload used for a missing project entry so the user can either
    // configure the original provider or open a copy with a ready one.
    if loaded_target.is_some() && !provider_is_selectable(&config, &provider_id) {
        return Err(provider_unavailable_error(
            &config,
            &provider_id,
            loaded_target
                .as_ref()
                .map(|loaded| loaded.thread.id.as_str()),
        ));
    }

    // Preflight the complete target before changing the active workspace. The
    // thread override lets start_session build the runtime without writing
    // target sticky state until the build has succeeded.
    let target_state_path = ProjectSessionState::path(&root);
    let previous_target_state = std::fs::read(&target_state_path).ok();
    let previous_last_provider = snapshot_last_provider();
    let mut created_thread_id: Option<String> = None;
    let target_thread = if new_thread {
        let thread = target_store
            .create_for_provider(&provider_id)
            .map_err(|e| e.to_string())?;
        created_thread_id = Some(thread.id.clone());
        Some((thread, None, false))
    } else if let Some(loaded) = loaded_target {
        let warning = loaded.warning;
        let source = loaded.thread;
        let thread = if copying_to_another_provider {
            target_store
                .fork_for_provider(&source, &provider_id, None)
                .map_err(|e| e.to_string())?
        } else {
            source
        };
        if copying_to_another_provider {
            created_thread_id = Some(thread.id.clone());
        }
        Some((thread, warning, false))
    } else {
        let resolved = resolve_thread(
            &root,
            &target_store,
            &provider_id,
            &config,
            explicit_provider.is_some(),
        )?;
        if resolved.created {
            created_thread_id = Some(resolved.thread.id.clone());
        }
        Some((resolved.thread, resolved.warning, resolved.draft))
    };

    let previous_space_state = state
        .space_state
        .lock()
        .map_err(|_| "Space state lock poisoned.".to_string())?
        .clone();
    if !free_chat {
        set_workspace_root(&state, root.clone())?;
    }
    if !free_chat {
        if let Err(error) = remember_active_space_workspace(&state, &root) {
            restore_snapshot(&target_state_path, previous_target_state.clone());
            if let Some(thread_id) = created_thread_id.as_ref() {
                let _ = target_store.delete(thread_id);
            }
            restore_last_provider(previous_last_provider.clone());
            restore_space_state(&state, previous_space_state.clone());
            if let Some(previous_root) = previous_root.as_ref() {
                let _ = set_workspace_root(&state, previous_root.clone());
            }
            return Err(error);
        }
    }

    // Keep the old route alive until the new runtime has been built. If it is
    // still running, its worker continues in the background; if it is idle,
    // `set_session` removes it after the new runtime is ready.
    let result = start_session_inner(
        state.clone(),
        provider_id.clone(),
        None,
        None,
        Some(root.clone()),
        target_thread,
    )
    .await;
    if result.is_err() {
        restore_snapshot(&target_state_path, previous_target_state);
        if let Some(thread_id) = created_thread_id {
            let _ = target_store.delete(&thread_id);
        }
        restore_last_provider(previous_last_provider);
        restore_space_state(&state, previous_space_state);
        if let Some(previous_root) = previous_root {
            let _ = set_workspace_root(&state, previous_root);
        }
    }

    result
}

#[tauri::command]
fn load_thread(state: State<'_, AppState>, id: String) -> Result<SessionInfo, String> {
    state.sessions.require_idle().map_err(map_session_err)?;

    let root = state
        .sessions
        .session_info_snapshot(|session| session.root.clone())
        .map_err(map_session_err)?
        .ok_or_else(|| desktop_err("no_session", "open a project before opening a chat"))?;
    let config = if is_free_chat_root(&root) {
        config_for_free_chat(&state, &root)?
    } else {
        config_for_session(&state, &root)?
    };

    state
        .sessions
        .with_session_mut(|session| -> Result<SessionInfo, String> {
            let store = open_store(&session.root)?;
            let loaded = store
                .load_for_provider(&id, &session.provider_id)
                .map_err(|e| e.to_string())?;
            if loaded.thread.provider_id.is_none() {
                return Err(thread_provider_unknown_error(&config, &loaded.thread.id));
            }
            let loaded_thread = loaded.thread;
            let load_warning = loaded.warning;
            let recovery_warning = match recover_chat_on_load(
                &session.root,
                &store,
                &loaded_thread.id,
                load_warning.is_some(),
            ) {
                Ok((recovered_thread, warning, recovery)) => {
                    session.agent.clear_messages();
                    session.agent.messages = recovered_thread.agent_messages.clone();
                    session.thread_id = recovered_thread.id.clone();
                    session.thread = recovered_thread;
                    session.recovery = recovery;
                    warning
                }
                Err(_) => {
                    session.agent.clear_messages();
                    session.agent.messages = loaded_thread.agent_messages.clone();
                    session.thread_id = loaded_thread.id.clone();
                    session.thread = loaded_thread;
                    session.recovery = None;
                    Some(
                        "Chat recovery state could not be checked; the saved transcript is still available."
                            .into(),
                    )
                }
            };
            persist_provider_thread(&session.root, &session.provider_id, &session.thread_id)?;
            Ok(session_info_from(
                session,
                merge_warnings(load_warning, recovery_warning.into_iter().collect()),
            ))
        })
        .map_err(map_session_err)
        .and_then(|r| r)
}

#[tauri::command]
fn new_thread(state: State<'_, AppState>) -> Result<SessionInfo, String> {
    state.sessions.require_idle().map_err(map_session_err)?;

    state
        .sessions
        .with_session_mut(|session| -> Result<SessionInfo, String> {
            let store = open_store(&session.root)?;
            let thread = store
                .create_for_provider(&session.provider_id)
                .map_err(|e| e.to_string())?;
            session.agent.clear_messages();
            session.thread_id = thread.id.clone();
            session.thread = thread;
            session.recovery = None;
            persist_provider_thread(&session.root, &session.provider_id, &session.thread_id)?;
            Ok(session_info_from(session, None))
        })
        .map_err(map_session_err)
        .and_then(|r| r)
}

/// Fork the active conversation into a new provider-owned thread. The runtime
/// options stay the same, while future checkpoints belong only to the fork.
#[tauri::command]
fn fork_thread(state: State<'_, AppState>) -> Result<SessionInfo, String> {
    state.sessions.require_idle().map_err(map_session_err)?;

    state
        .sessions
        .with_session_mut(|session| -> Result<SessionInfo, String> {
            let store = open_store(&session.root)?;
            let mut fork = store
                .fork(&session.thread, None)
                .map_err(|e| e.to_string())?;
            fork.provider_session = None;
            session.agent.clear_messages();
            session.agent.messages = fork.agent_messages.clone();
            session.agent.last_usage = None;
            session.thread_id = fork.id.clone();
            session.thread = fork;
            session.recovery = None;
            persist_provider_thread(&session.root, &session.provider_id, &session.thread_id)?;
            Ok(session_info_from(
                session,
                Some("A new conversation was created from this one.".into()),
            ))
        })
        .map_err(map_session_err)
        .and_then(|r| r)
}

/// Fork from a saved checkpoint without changing the original conversation.
/// Provider-native cursors are intentionally discarded so the new chat is
/// rehydrated from its canonical checkpoint transcript.
#[tauri::command]
fn fork_thread_from_checkpoint(
    state: State<'_, AppState>,
    checkpoint_id: String,
) -> Result<SessionInfo, String> {
    state.sessions.require_idle().map_err(map_session_err)?;

    state
        .sessions
        .with_session_mut(|session| -> Result<SessionInfo, String> {
            let store = open_store(&session.root)?;
            let mut fork = store
                .fork_from_checkpoint(&session.thread, checkpoint_id.trim(), None)
                .map_err(|e| e.to_string())?;
            fork.provider_session = None;
            session.agent.clear_messages();
            session.agent.messages = fork.agent_messages.clone();
            session.agent.last_usage = None;
            session.thread_id = fork.id.clone();
            session.thread = fork;
            session.recovery = None;
            persist_provider_thread(&session.root, &session.provider_id, &session.thread_id)?;
            Ok(session_info_from(
                session,
                Some("A new conversation was forked from this checkpoint.".into()),
            ))
        })
        .map_err(map_session_err)
        .and_then(|r| r)
}

/// Restore the active conversation to a durable checkpoint. Workspace files
/// are intentionally untouched: this first version is a safe conversation
/// rewind, not an implicit filesystem reset.
#[tauri::command]
fn rewind_thread(state: State<'_, AppState>, checkpoint_id: String) -> Result<SessionInfo, String> {
    state.sessions.require_idle().map_err(map_session_err)?;

    state
        .sessions
        .with_session_mut(|session| -> Result<SessionInfo, String> {
            let store = open_store(&session.root)?;
            let mut restored = store
                .rewind_to_checkpoint(&session.thread, checkpoint_id.trim())
                .map_err(|e| e.to_string())?;
            restored.provider_session = None;
            restored
                .assert_provider(&session.provider_id)
                .map_err(|e| e.to_string())?;
            session.agent.clear_messages();
            session.agent.messages = restored.agent_messages.clone();
            session.agent.last_usage = None;
            session.agent.clear_provider_session();
            restored.provider_session = None;
            session.thread = restored;
            session.thread_id = session.thread.id.clone();
            session.recovery = None;
            store.save(&session.thread).map_err(|e| e.to_string())?;
            persist_provider_thread(&session.root, &session.provider_id, &session.thread_id)?;
            Ok(session_info_from(
                session,
                Some("Conversation restored. Your files were not changed.".into()),
            ))
        })
        .map_err(map_session_err)
        .and_then(|r| r)
}

/// Read the cumulative branch/worktree patch for the active chat. This is
/// local-only; the raw patch is never submitted to a provider by this command.
#[tauri::command]
async fn workspace_changes(state: State<'_, AppState>) -> Result<WorkspaceChangeView, String> {
    let snapshot = state
        .sessions
        .session_info_snapshot(|session| (session.root.clone(), session.thread.git_context.clone()))
        .map_err(map_session_err)?
        .ok_or_else(|| desktop_err("no_session", "no active chat"))?;
    let (root, context) = snapshot;
    let changes = zest_core::workspace_changes::inspect(
        root,
        context
            .as_ref()
            .and_then(|context| context.start_commit.as_deref()),
        context
            .as_ref()
            .and_then(|context| context.base_branch.as_deref()),
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(changes.into())
}

/// Remove a user message and its later branch so the UI can submit an edited
/// replacement as a fresh turn. Workspace files are intentionally untouched.
#[tauri::command]
fn edit_message(state: State<'_, AppState>, message_id: String) -> Result<SessionInfo, String> {
    state.sessions.require_idle().map_err(map_session_err)?;

    let message_id = message_id.trim().to_string();
    if message_id.is_empty() {
        return Err(desktop_err("invalid", "message id is empty"));
    }

    state
        .sessions
        .with_session_mut(|session| -> Result<SessionInfo, String> {
            let store = open_store(&session.root)?;
            let mut restored = store
                .rewind_before_user_message(&session.thread, &message_id)
                .map_err(|e| e.to_string())?;
            restored
                .assert_provider(&session.provider_id)
                .map_err(|e| e.to_string())?;

            session.agent.clear_messages();
            session.agent.messages = restored.agent_messages.clone();
            session.agent.last_usage = None;
            session.agent.clear_provider_session();
            restored.provider_session = None;
            session.thread = restored;
            session.recovery = None;
            persist_provider_thread(&session.root, &session.provider_id, &session.thread_id)?;
            Ok(session_info_from(session, None))
        })
        .map_err(map_session_err)
        .and_then(|r| r)
}

/// Ask the active provider for a compact, persistence-safe checkpoint of the
/// conversation. The operation occupies the normal turn slot so it cannot race
/// a send or an approval, but it does not add a visible assistant answer.
#[tauri::command]
async fn compact_context(state: State<'_, AppState>) -> Result<CompactionResultView, String> {
    state.sessions.require_idle().map_err(map_session_err)?;

    let (mut session, turn) = state.sessions.begin_turn().map_err(map_session_err)?;
    let store = match open_store(&session.root) {
        Ok(store) => store,
        Err(error) => {
            let _ = state.sessions.finish_turn(&turn, session);
            return Err(error);
        }
    };
    if session.thread.messages.is_empty() && session.agent.messages.len() < 4 {
        let _ = state.sessions.finish_turn(&turn, session);
        return Err("there is not enough conversation to compact yet".into());
    }
    if let Err(error) = store.create_checkpoint_with_metadata(
        &mut session.thread,
        "Before compaction",
        None,
        Some("Context compaction".into()),
        ThreadCheckpointKind::Compaction,
    ) {
        let _ = state.sessions.finish_turn(&turn, session);
        return Err(error.to_string());
    }

    let result = session.agent.compact_context().await;
    let output = match result {
        // Both paths rewrote history, so both need the same persistence handling.
        // The checkpoint written above is kept either way: the UI transcript
        // stores only a short summary per tool call, so that snapshot's
        // `agent_messages` is the one durable copy of the full tool bodies a
        // prune just shortened.
        Ok(outcome) => {
            let (pruned_only, results_pruned) = match &outcome {
                CompactionOutcome::Pruned { results_pruned, .. } => (true, *results_pruned),
                CompactionOutcome::Summarized { .. } => (false, 0),
            };
            session.thread.provider_session = None;
            session
                .thread
                .set_agent_messages(session.agent.messages_for_persist());
            if let Err(error) = store.save(&session.thread) {
                let _ = state.sessions.finish_turn(&turn, session);
                return Err(error.to_string());
            }
            CompactionResultView {
                usage: estimate_context(&session.agent, session.thread.checkpoints.len()),
                pruned_only,
                results_pruned,
            }
        }
        Err(error) => {
            session.agent.clear_provider_session();
            session.thread.provider_session = None;
            let _ = store.save(&session.thread);
            let _ = state.sessions.finish_turn(&turn, session);
            return Err(error.to_string());
        }
    };
    let _ = state.sessions.finish_turn(&turn, session);
    Ok(output)
}

/// Delete a saved chat. If it is the active thread, switches the session to an
/// unsaved empty draft for the same provider. The draft becomes a saved chat
/// when its first message is persisted. `project_path` deletes from another
/// known project without switching the open workspace.
#[tauri::command]
fn delete_thread(
    state: State<'_, AppState>,
    id: String,
    project_path: Option<String>,
    free_chat: Option<bool>,
) -> Result<SessionInfo, String> {
    state.sessions.require_idle().map_err(map_session_err)?;

    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(desktop_err("invalid", "chat id is empty"));
    }

    let target_root = if free_chat.unwrap_or(false) {
        free_chats_root()?
    } else {
        match project_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(raw) => canonicalize_dir(PathBuf::from(raw))?,
            None => state
                .sessions
                .active_root()
                .map_err(map_session_err)?
                .ok_or_else(|| desktop_err("no_session", "open a chat before deleting a chat"))?,
        }
    };

    // A background turn still owns the authoritative transcript. Refuse to
    // delete it from a different route, otherwise its final save could
    // recreate the chat after the sidebar reports success.
    if state
        .sessions
        .active_turn_for_thread(&id)
        .map_err(map_session_err)?
        .is_some_and(|turn| {
            turn.root == target_root || display_path(&turn.root) == display_path(&target_root)
        })
    {
        return Err(desktop_err(
            "busy",
            "this chat is still working — stop it before deleting it",
        ));
    }

    state
        .sessions
        .with_session_mut(|session| -> Result<SessionInfo, String> {
            let store = open_store(&target_root)?;
            // Deletion is allowed across providers: the sidebar intentionally
            // lists every provider's chats, and removing a chat does not
            // restore or execute it. Reopening still uses load_for_provider
            // and therefore keeps the cross-provider safety boundary.
            let _ = store.load(&id).map_err(|e| e.to_string())?;
            store.delete(&id).map_err(|e| e.to_string())?;

            // The transcript was only part of what this chat left behind. Its
            // run and interrupt records lived on in the project, read on every
            // open and never collected, describing a conversation the user
            // asked to be rid of. Best effort, and after the thread is gone: a
            // side record that will not unlink is no reason to report the
            // deletion as failed.
            if let Ok(persistence) = ChatPersistence::open(&target_root) {
                let _ = persistence.forget_thread(&id);
            }

            // Compare via display paths — `session.root` may be `\\?\…` while the
            // sidebar sends a stripped path that still canonicalizes differently.
            let same_project = display_path(&session.root) == display_path(&target_root)
                || session.root == target_root;
            if same_project && session.thread_id == id {
                let thread = Thread::new().with_provider(&session.provider_id);
                session.agent.clear_messages();
                session.thread_id = thread.id.clone();
                session.thread = thread;
                session.recovery = None;
                // Keep the active provider pointing at the draft, but do not
                // create a history row until the user sends a message. The
                // pointer is what lets `resolve_thread` hand this same draft
                // back if the project is reopened before anything is sent.
                persist_provider_thread(&session.root, &session.provider_id, &session.thread_id)?;
            }
            Ok(session_info_from(session, None))
        })
        .map_err(map_session_err)
        .and_then(|r| r)
}

/// Set a chat's sidebar pin without changing its activity timestamp. This is
/// allowed for any known project because pinning only changes navigation
/// metadata; it never opens, rewinds, or executes the conversation.
#[tauri::command]
fn set_thread_pinned(
    state: State<'_, AppState>,
    id: String,
    project_path: Option<String>,
    free_chat: Option<bool>,
    pinned: bool,
) -> Result<(), String> {
    state.sessions.require_idle().map_err(map_session_err)?;

    let target_root = state
        .sessions
        .with_session_mut(|session| -> Result<PathBuf, String> {
            let target_root = if free_chat.unwrap_or(false) {
                free_chats_root()?
            } else {
                match project_path
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    Some(raw) => canonicalize_dir(PathBuf::from(raw))?,
                    None => session.root.clone(),
                }
            };
            let store = open_store(&target_root)?;
            let summary = store.set_pinned(&id, pinned).map_err(|e| e.to_string())?;

            let same_project = display_path(&session.root) == display_path(&target_root)
                || session.root == target_root;
            if same_project && session.thread_id == id {
                session.thread.pinned = summary.pinned;
            }
            Ok(target_root)
        })
        .map_err(map_session_err)
        .and_then(|result| result)?;

    // The scanner normally invalidates from file metadata. Remove the entry
    // explicitly as well so a fast pin/unpin always refreshes immediately on
    // filesystems with coarse timestamp resolution.
    if let Ok(mut cache) = state.chat_summary_cache.lock() {
        cache.projects.remove(&target_root);
    }
    Ok(())
}

/// Rename a saved chat without changing its activity timestamp. This is
/// allowed for any known project because renaming only changes navigation
/// metadata; it never opens, rewinds, or executes the conversation.
#[tauri::command]
fn rename_thread(
    state: State<'_, AppState>,
    id: String,
    project_path: Option<String>,
    free_chat: Option<bool>,
    title: String,
) -> Result<ThreadSummary, String> {
    state.sessions.require_idle().map_err(map_session_err)?;

    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(desktop_err("invalid", "chat id is empty"));
    }
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err(desktop_err("invalid", "chat title is empty"));
    }

    let (target_root, summary) = state
        .sessions
        .with_session_mut(|session| -> Result<(PathBuf, ThreadSummary), String> {
            let target_root = if free_chat.unwrap_or(false) {
                free_chats_root()?
            } else {
                match project_path
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    Some(raw) => canonicalize_dir(PathBuf::from(raw))?,
                    None => session.root.clone(),
                }
            };
            let store = open_store(&target_root)?;
            let summary = store.rename(&id, &title).map_err(|e| e.to_string())?;

            let same_project = display_path(&session.root) == display_path(&target_root)
                || session.root == target_root;
            if same_project && session.thread_id == id {
                session.thread.title = summary.title.clone();
            }
            Ok((target_root, summary))
        })
        .map_err(map_session_err)
        .and_then(|result| result)?;

    // Remove the cached summary explicitly so a rename is visible immediately
    // even on filesystems with coarse timestamp resolution.
    if let Ok(mut cache) = state.chat_summary_cache.lock() {
        cache.projects.remove(&target_root);
    }
    Ok(summary)
}

#[tauri::command]
async fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    text: String,
    attachments: Option<Vec<AttachmentInput>>,
) -> Result<(), String> {
    turn::run(app, state.inner(), text, attachments).await
}
#[tauri::command]
fn cancel_turn(state: State<'_, AppState>, thread_id: Option<String>) -> Result<(), String> {
    // Cancel token first so in-flight select! races abort before waiters clear.
    let requested_thread = thread_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    let active_turn = match requested_thread {
        Some(id) => state
            .sessions
            .active_turn_for_thread(id)
            .map_err(map_session_err)?,
        None => state.sessions.active_turn().map_err(map_session_err)?,
    };
    let cancelled = match requested_thread {
        Some(id) => state
            .sessions
            .cancel_turn_for_thread(id)
            .map_err(map_session_err)?,
        None => state.sessions.cancel_turn().map_err(map_session_err)?,
    };
    if !cancelled {
        return Err(desktop_err("no_turn", "no turn in progress"));
    }
    if let Some(turn) = active_turn {
        turn.approval_hub.clear();
        turn.question_hub.clear();
        if let Ok(persistence) = ChatPersistence::open(&turn.root) {
            let _ = persistence.interrupts.cancel_pending_by_run(&turn.turn_id);
            let _ = persistence.runs.mark_aborted(&turn.turn_id);
        }
    }
    Ok(())
}

#[tauri::command]
fn resolve_approval(
    state: State<'_, AppState>,
    approval_id: String,
    decision: String,
    thread_id: Option<String>,
) -> Result<(), String> {
    // Unknown strings deny rather than default to allow: a UI/backend version
    // skew must fail closed.
    let decision = match decision.as_str() {
        "once" => ApprovalDecision::AllowOnce,
        "session" => ApprovalDecision::AllowSession,
        "deny" => ApprovalDecision::Deny,
        other => return Err(format!("unknown approval decision `{other}`")),
    };
    let persisted_decision = match decision {
        ApprovalDecision::AllowOnce => "once",
        ApprovalDecision::AllowSession => "session",
        ApprovalDecision::Deny => "deny",
    };
    let active_turn = match thread_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        Some(id) => state
            .sessions
            .active_turn_for_thread(id)
            .map_err(map_session_err)?,
        None => state.sessions.active_turn().map_err(map_session_err)?,
    };
    let result = active_turn
        .as_ref()
        .ok_or_else(|| desktop_err("no_turn", "no turn in progress"))?
        .approval_hub
        .resolve(&approval_id, decision);
    if result.is_ok() {
        if let Some(turn) = active_turn {
            if let Ok(persistence) = ChatPersistence::open(&turn.root) {
                let _ = persistence.interrupts.resolve(
                    &approval_id,
                    Some(json!({ "decision": persisted_decision })),
                );
                let _ = persistence.runs.mark_running(&turn.turn_id);
            }
        }
    }
    result
}

/// Deliver a structured questionnaire answer to the active `ask_user` tool.
/// The answer resumes the existing turn; it does not start a second turn.
#[tauri::command]
fn resolve_question(
    state: State<'_, AppState>,
    question_id: String,
    answer: String,
    thread_id: Option<String>,
) -> Result<(), String> {
    let answer = answer.trim().to_string();
    if answer.is_empty() {
        return Err("answer cannot be empty".into());
    }
    let question_id = question_id.trim().to_string();
    let active_turn = match thread_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        Some(id) => state
            .sessions
            .active_turn_for_thread(id)
            .map_err(map_session_err)?,
        None => state.sessions.active_turn().map_err(map_session_err)?,
    };
    let result = active_turn
        .as_ref()
        .ok_or_else(|| desktop_err("no_turn", "no turn in progress"))?
        .question_hub
        .resolve(&question_id, answer.clone());
    if result.is_ok() {
        if let Some(turn) = active_turn {
            if let Ok(persistence) = ChatPersistence::open(&turn.root) {
                let _ = persistence
                    .interrupts
                    .resolve(&question_id, Some(json!({ "answer": answer })));
                let _ = persistence.runs.mark_running(&turn.turn_id);
            }
        }
    }
    result
}

/// Switch the permission mode for the live session.
///
/// Grants made under the previous mode are dropped by `ApprovalPolicy`. The
/// policy outlives any one project, so switching folders keeps the mode.
#[tauri::command]
fn set_approval_mode(state: State<'_, AppState>, mode: String) -> Result<String, String> {
    let mode = ApprovalMode::parse(&mode).ok_or_else(|| format!("unknown mode `{mode}`"))?;
    state
        .policy
        .lock()
        .map_err(|_| "approval policy lock poisoned".to_string())?
        .set_mode(mode);
    Ok(mode.as_str().to_string())
}

#[tauri::command]
fn approval_mode(state: State<'_, AppState>) -> Result<String, String> {
    let mode = state
        .policy
        .lock()
        .map_err(|_| "approval policy lock poisoned".to_string())?
        .mode();
    Ok(mode.as_str().to_string())
}

#[tauri::command]
async fn generate_reading_diff(
    state: State<'_, AppState>,
    diff: String,
) -> Result<ReadingDiffView, String> {
    let snapshot = state
        .sessions
        .session_info_snapshot(|session| {
            (
                session.agent.provider(),
                session.model.clone(),
                session.effort.clone(),
            )
        })
        .map_err(map_session_err)?
        .ok_or_else(|| {
            desktop_err(
                "no_session",
                "open a provider before generating a reading diff",
            )
        })?;
    let result = zest_core::abridge_reading_diff(snapshot.0, &snapshot.1, &snapshot.2, &diff)
        .await
        .map_err(|e| desktop_err("reading_diff", e.to_string()))?;
    Ok(ReadingDiffView {
        diff: result.diff,
        summary: result.summary,
        removed_lines: result.removed_lines,
        folded_lines: result.folded_lines,
    })
}

#[tauri::command]
fn end_session(state: State<'_, AppState>) -> Result<(), String> {
    // Cancel only the route currently shown. Turns in other chats remain
    // registered and continue in the background.
    let active_turn = state.sessions.active_turn().map_err(map_session_err)?;
    state.sessions.end_session().map_err(map_session_err)?;
    if let Some(turn) = active_turn {
        turn.approval_hub.clear();
        turn.question_hub.clear();
    }
    Ok(())
}

#[tauri::command]
fn session_info(state: State<'_, AppState>) -> Option<SessionInfo> {
    state
        .sessions
        .session_info_snapshot(|s| session_info_from(s, None))
        .ok()
        .flatten()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemPromptInfo {
    base: String,
    custom: String,
    /// Truncated composed preview for the Settings UI.
    composed_preview: String,
    custom_path: String,
}

const COMPOSED_PREVIEW_MAX: usize = 2400;

#[tauri::command]
fn get_system_prompt(state: State<'_, AppState>) -> Result<SystemPromptInfo, String> {
    state
        .sessions
        .with_session_mut(|session| system_prompt_info(session))
        .map_err(map_session_err)
        .and_then(|r| r)
}

#[tauri::command]
fn set_system_prompt(
    state: State<'_, AppState>,
    custom: String,
) -> Result<SystemPromptInfo, String> {
    state.sessions.require_idle().map_err(map_session_err)?;
    state
        .sessions
        .with_session_mut(|session| {
            save_custom_system(&session.root, &custom).map_err(|e| e.to_string())?;
            let skills = SkillSet::discover();
            {
                let mut guard = session
                    .skills
                    .write()
                    .map_err(|_| "skill registry lock poisoned".to_string())?;
                *guard = skills;
            }
            // Must mirror RuntimeBuilder::build exactly — docs and environment
            // included — or saving Settings would quietly strip them.
            let composed = {
                let guard = session
                    .skills
                    .read()
                    .map_err(|_| "skill registry lock poisoned".to_string())?;
                let docs = load_project_docs(&session.root);
                let body = compose_system_with_docs(&session.base_system, &custom, &docs, &guard);
                // Same split as RuntimeBuilder::build: environment after the
                // cache breakpoint, not concatenated into the cached block.
                SystemPrompt::new(body).with_volatile(env_context(&session.root))
            };
            session.agent.system = Some(composed);
            system_prompt_info(session)
        })
        .map_err(map_session_err)
        .and_then(|r| r)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "export-bindings", derive(TS))]
#[cfg_attr(
    feature = "export-bindings",
    ts(export, export_to = "CommandView.ts", rename_all = "camelCase")
)]
pub struct CommandView {
    pub name: String,
    pub description: String,
}

/// Slash commands available here — one per discovered skill.
///
/// Disk-based so commands remain readable while a turn is streaming.
#[tauri::command]
fn list_commands(_state: State<'_, AppState>) -> Result<Vec<CommandView>, String> {
    Ok(SkillSet::discover()
        .command_names()
        .into_iter()
        .map(|(name, description)| CommandView { name, description })
        .collect())
}

#[tauri::command]
/// Discovered from the user's skill folders rather than the live session.
///
/// `begin_turn` *takes* the session out of the controller, so anything that
/// reaches through it is unreadable while a turn runs — which made opening
/// Settings mid-turn fail with "a turn is already in progress". Skills come
/// from disk, and disk is readable whenever.
fn list_skills(_state: State<'_, AppState>) -> Result<Vec<SkillSummary>, String> {
    Ok(SkillSet::discover().summaries())
}

fn system_prompt_info(session: &Session) -> Result<SystemPromptInfo, String> {
    let custom = load_custom_system(&session.root)?;
    // The whole prompt as the model reads it — the preview would be lying if
    // it showed only the half that happens to be cacheable.
    let composed = session
        .agent
        .system
        .as_ref()
        .map(SystemPrompt::text)
        .unwrap_or_else(|| session.base_system.clone());
    let composed_preview = truncate_chars(&composed, COMPOSED_PREVIEW_MAX);
    Ok(SystemPromptInfo {
        base: session.base_system.clone(),
        custom,
        composed_preview,
        custom_path: display_path(&session.root.join(".zest").join("system.md")),
    })
}

fn zest_config_dir() -> Result<std::path::PathBuf, String> {
    let dir = dirs::config_dir()
        .ok_or_else(|| "no config directory".to_string())?
        .join("zest");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

const SPACES_FILE: &str = "spaces.json";

fn spaces_state_path() -> Result<PathBuf, String> {
    Ok(zest_config_dir()?.join(SPACES_FILE))
}

fn load_space_state() -> SpaceState {
    spaces_state_path()
        .map(|path| SpaceState::load(&path))
        .unwrap_or_default()
}

fn save_space_state(state: &SpaceState) -> Result<(), String> {
    state.save(&spaces_state_path()?)
}

const MARKDOWN_SAVE_DIRECTORY_FILE: &str = "last-markdown-save-directory";

fn markdown_save_directory_path() -> Result<PathBuf, String> {
    Ok(zest_config_dir()?.join(MARKDOWN_SAVE_DIRECTORY_FILE))
}

fn load_markdown_save_directory() -> Option<PathBuf> {
    let path = markdown_save_directory_path().ok()?;
    let raw = std::fs::read_to_string(path).ok()?;
    let directory = PathBuf::from(raw.trim());
    directory.is_dir().then_some(directory)
}

fn persist_markdown_save_directory(directory: &Path) -> Result<(), String> {
    let path = markdown_save_directory_path()?;
    std::fs::write(path, display_path(directory)).map_err(|e| e.to_string())
}

fn choose_markdown_save_directory(workspace: PathBuf, remembered: Option<PathBuf>) -> PathBuf {
    remembered
        .filter(|directory| directory.is_dir())
        .unwrap_or(workspace)
}

fn sanitize_markdown_filename(value: &str) -> String {
    let trimmed = value.trim();
    let without_extension = trimmed
        .strip_suffix(".md")
        .or_else(|| trimmed.strip_suffix(".MD"))
        .unwrap_or(trimmed);
    let mut safe = without_extension
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
            {
                '-'
            } else if character.is_whitespace() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    safe = safe.trim().trim_end_matches(['.', ' ']).to_string();
    if safe.is_empty() {
        safe = "response".into();
    }
    let uppercase = safe.to_ascii_uppercase();
    let device_name = matches!(uppercase.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (uppercase.len() == 4
            && (uppercase.starts_with("COM") || uppercase.starts_with("LPT"))
            && uppercase
                .chars()
                .nth(3)
                .is_some_and(|character| character.is_ascii_digit()));
    if device_name {
        safe.insert(0, '_');
    }
    safe.chars().take(120).collect::<String>() + ".md"
}

fn enforce_markdown_extension(mut path: PathBuf) -> PathBuf {
    let is_markdown = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"));
    if !is_markdown {
        path.set_extension("md");
    }
    path
}

fn write_markdown_file(path: &Path, markdown: &str) -> Result<(), String> {
    zest_core::atomic_write(path, markdown.as_bytes()).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_markdown(
    state: State<'_, AppState>,
    suggested_name: String,
    markdown: String,
) -> Result<Option<String>, String> {
    let workspace = resolve_workspace_root(&state)?;
    let directory = choose_markdown_save_directory(workspace, load_markdown_save_directory());
    let filename = sanitize_markdown_filename(&suggested_name);
    let dialog = rfd::FileDialog::new()
        .set_title("Save Markdown")
        .add_filter("Markdown", &["md"])
        .set_file_name(&filename)
        .set_directory(directory);
    let Some(selected_path) = dialog.save_file() else {
        return Ok(None);
    };
    let path = enforce_markdown_extension(selected_path);
    write_markdown_file(&path, &markdown)?;
    if let Some(parent) = path.parent() {
        persist_markdown_save_directory(parent)?;
    }
    Ok(Some(display_path(&path)))
}

#[cfg(test)]
mod markdown_export_tests {
    use super::*;

    #[test]
    fn sanitizes_names_and_enforces_markdown_extension() {
        assert_eq!(
            sanitize_markdown_filename("Roadmap: <draft>?.md"),
            "Roadmap- -draft--.md"
        );
        assert_eq!(sanitize_markdown_filename("CON"), "_CON.md");
        assert_eq!(
            enforce_markdown_extension(PathBuf::from("answer.txt")),
            PathBuf::from("answer.md")
        );
        assert_eq!(
            enforce_markdown_extension(PathBuf::from("answer.MD")),
            PathBuf::from("answer.MD")
        );
    }

    #[test]
    fn remembers_a_valid_directory_and_falls_back_for_missing_one() {
        let base = std::env::temp_dir().join(format!("zest-markdown-dir-{}", new_id("test")));
        let workspace = base.join("workspace");
        let remembered = base.join("remembered");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&remembered).unwrap();

        assert_eq!(
            choose_markdown_save_directory(workspace.clone(), Some(remembered.clone())),
            remembered
        );
        assert_eq!(
            choose_markdown_save_directory(workspace.clone(), Some(base.join("gone"))),
            workspace
        );
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn reports_atomic_write_failures() {
        let base = std::env::temp_dir().join(format!("zest-markdown-write-{}", new_id("test")));
        std::fs::create_dir_all(&base).unwrap();
        let parent_file = base.join("not-a-directory");
        std::fs::write(&parent_file, "occupied").unwrap();
        let result = write_markdown_file(&parent_file.join("answer.md"), "# answer");
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(base);
    }
}

fn persist_choice(id: &str) -> Result<(), String> {
    let path = zest_config_dir()?.join("last-provider");
    std::fs::write(&path, id).map_err(|e| e.to_string())?;
    Ok(())
}

fn snapshot_last_provider() -> Option<(PathBuf, Option<Vec<u8>>)> {
    let path = dirs::config_dir()?.join("zest").join("last-provider");
    Some((path.clone(), std::fs::read(path).ok()))
}

fn restore_last_provider(snapshot: Option<(PathBuf, Option<Vec<u8>>)>) {
    let Some((path, contents)) = snapshot else {
        return;
    };
    restore_snapshot(&path, contents);
}

fn restore_snapshot(path: &Path, contents: Option<Vec<u8>>) {
    match contents {
        Some(contents) => {
            let _ = std::fs::write(path, contents);
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[tauri::command]
fn last_provider() -> Option<String> {
    let path = dirs::config_dir()?.join("zest").join("last-provider");
    let value = std::fs::read_to_string(path).ok()?;
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

#[tauri::command]
fn get_workspace_folder(state: State<'_, AppState>) -> Result<String, String> {
    Ok(display_path(&resolve_workspace_root(&state)?))
}

/// List one project directory for the Workbench file browser.
///
/// This is intentionally shallow: the UI can ask for a child directory after
/// the user opens it, which avoids walking a large repository on every render.
#[tauri::command]
async fn list_workspace_files(
    state: State<'_, AppState>,
    relative_path: Option<String>,
) -> Result<Vec<WorkspaceFileView>, String> {
    let root = resolve_workspace_root(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        workspace_files::list(&root, relative_path.as_deref())
    })
    .await
    .map_err(|error| format!("could not list workspace files: {error}"))?
}

#[tauri::command]
async fn read_workspace_file(
    state: State<'_, AppState>,
    relative_path: String,
) -> Result<WorkspaceFileContent, String> {
    let root = resolve_workspace_root(&state)?;
    tauri::async_runtime::spawn_blocking(move || workspace_files::read(&root, &relative_path))
        .await
        .map_err(|error| format!("could not read workspace file: {error}"))?
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspacePickResult {
    path: String,
    /// True when an open session was closed so the UI must start a new one.
    session_ended: bool,
}

/// Native folder picker. Stores preference for the next `start_session`.
/// Returns `null` when the user cancels. Ends an idle open session; a running
/// chat stays registered so its turn can finish against its original root.
#[tauri::command]
fn pick_workspace_folder(
    state: State<'_, AppState>,
) -> Result<Option<WorkspacePickResult>, String> {
    let mut dialog = rfd::FileDialog::new().set_title("Open project folder");
    if let Ok(current) = resolve_workspace_root(&state) {
        dialog = dialog.set_directory(current);
    }
    let Some(folder) = dialog.pick_folder() else {
        return Ok(None);
    };
    let root = set_workspace_root(&state, folder)?;
    adopt_workspace_into_active_space(&state, &root)?;
    let had_session = state
        .sessions
        .has_active_session()
        .map_err(map_session_err)?;
    let was_busy = state.sessions.is_busy().map_err(map_session_err)?;
    let session_ended = if had_session && !was_busy {
        state.sessions.end_session().map_err(map_session_err)?;
        true
    } else {
        false
    };
    Ok(Some(WorkspacePickResult {
        path: display_path(&root),
        session_ended,
    }))
}

/// Native multi-file picker. PDFs are extracted via pdf-inspector.
#[tauri::command]
fn pick_files(state: State<'_, AppState>) -> Result<Vec<PreparedAttachment>, String> {
    let mut dialog = rfd::FileDialog::new()
        .set_title("Attach files")
        .add_filter(
            "Documents",
            &[
                "pdf", "md", "txt", "rs", "ts", "tsx", "js", "jsx", "json", "toml", "yaml", "yml",
                "py", "go", "java", "c", "h", "cpp", "cs", "html", "css", "svg", "csv", "log",
            ],
        )
        .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"])
        .add_filter("PDF", &["pdf"])
        .add_filter("All files", &["*"]);
    if let Ok(current) = resolve_workspace_root(&state) {
        dialog = dialog.set_directory(current);
    }
    let Some(paths) = dialog.pick_files() else {
        return Ok(Vec::new());
    };
    Ok(prepare_paths(&paths))
}

/// Paste / drop path: raw image bytes from the webview (base64).
#[tauri::command]
fn prepare_pasted_image(
    data_base64: String,
    media_type: String,
    name: Option<String>,
) -> Result<PreparedAttachment, String> {
    let raw = data_base64
        .split(',')
        .next_back()
        .unwrap_or(data_base64.as_str());
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .map_err(|e| format!("invalid image data: {e}"))?;
    let name = name.unwrap_or_else(|| {
        let ext = match media_type.to_ascii_lowercase().as_str() {
            "image/jpeg" | "image/jpg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => "png",
        };
        format!("paste-{}.{}", zest_core::new_id("img"), ext)
    });
    Ok(prepare_image_bytes(&bytes, &media_type, &name))
}

#[tauri::command]
fn git_branch(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let root = resolve_workspace_root(&state)?;
    Ok(read_git_branch(&root))
}

fn read_git_branch(root: &Path) -> Option<String> {
    let head = root.join(".git").join("HEAD");
    let contents = std::fs::read_to_string(head).ok()?;
    let line = contents.lines().next()?.trim();
    if let Some(branch) = line.strip_prefix("ref: refs/heads/") {
        return Some(branch.to_string());
    }
    // Detached HEAD — short hash.
    if line.len() >= 7 && line.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(format!("{}…", &line[..7]));
    }
    None
}

async fn read_git_head(root: &Path) -> Option<String> {
    let mut command = Command::new("git");
    command
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(root)
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!commit.is_empty()).then_some(commit)
}

#[derive(Debug, Default)]
struct GitDiffStats {
    additions: u64,
    deletions: u64,
    changed_files: u64,
}

fn parse_git_numstat(output: &[u8]) -> GitDiffStats {
    let mut stats = GitDiffStats::default();
    for line in String::from_utf8_lossy(output).lines() {
        let mut fields = line.split('\t');
        let additions = fields.next().and_then(|value| value.parse::<u64>().ok());
        let deletions = fields.next().and_then(|value| value.parse::<u64>().ok());
        // Binary diffs report `-` for both counts. They still represent one
        // changed file, but do not have meaningful line statistics.
        if additions.is_some() || deletions.is_some() || line.contains("\t-\t") {
            stats.changed_files = stats.changed_files.saturating_add(1);
        }
        stats.additions = stats.additions.saturating_add(additions.unwrap_or(0));
        stats.deletions = stats.deletions.saturating_add(deletions.unwrap_or(0));
    }
    stats
}

async fn read_git_diff_stats(root: &Path, start_commit: Option<&str>) -> GitDiffStats {
    let start_commit = start_commit.map(str::trim);
    let mut command = Command::new("git");
    command.args(["diff", "--numstat"]);
    if let Some(start_commit) = start_commit {
        if !zest_core::workspace_changes::is_safe_commit_id(start_commit) {
            return GitDiffStats::default();
        }
        command.arg(start_commit);
    }
    command.arg("--");
    command
        .current_dir(root)
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(Duration::from_secs(30), command.output()).await {
        Ok(Ok(output)) if output.status.success() => output,
        _ => return GitDiffStats::default(),
    };
    parse_git_numstat(&output.stdout)
}

#[derive(Debug, Deserialize)]
struct GitHubPullRequest {
    number: u64,
    title: String,
    state: String,
    #[serde(rename = "isDraft")]
    is_draft: bool,
    url: String,
    additions: u64,
    deletions: u64,
    #[serde(rename = "changedFiles")]
    changed_files: u64,
}

enum PullRequestLookup {
    Found(PullRequestLink),
    NotFound,
    Unavailable,
}

async fn lookup_pull_request(root: &Path) -> PullRequestLookup {
    let output = match tokio::time::timeout(
        Duration::from_secs(3),
        Command::new("gh")
            .args([
                "pr",
                "view",
                "--json",
                "number,title,state,isDraft,url,additions,deletions,changedFiles",
            ])
            .current_dir(root)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    {
        Ok(Ok(output)) => output,
        Ok(Err(_)) | Err(_) => return PullRequestLookup::Unavailable,
    };

    if !output.status.success() {
        return PullRequestLookup::NotFound;
    }
    let Ok(pr) = serde_json::from_slice::<GitHubPullRequest>(&output.stdout) else {
        return PullRequestLookup::Unavailable;
    };
    PullRequestLookup::Found(PullRequestLink {
        repository: None,
        number: pr.number,
        title: pr.title,
        url: pr.url,
        state: pr.state,
        is_draft: pr.is_draft,
        additions: pr.additions,
        deletions: pr.deletions,
        changed_files: pr.changed_files,
    })
}

struct GitContextInspection {
    thread_context: ThreadGitContext,
    view: GitContextView,
}

async fn inspect_git_context(
    root: &Path,
    existing: Option<&ThreadGitContext>,
) -> GitContextInspection {
    let current_branch = read_git_branch(root);
    let mut thread_context = existing.cloned().unwrap_or_default();
    if thread_context.base_branch.is_none() {
        thread_context.base_branch = current_branch.clone();
    }
    if thread_context.start_commit.is_none() {
        thread_context.start_commit = read_git_head(root).await;
    }
    if current_branch.is_some() {
        thread_context.branch = current_branch.clone();
    }

    let local_stats = read_git_diff_stats(root, thread_context.start_commit.as_deref()).await;
    let mut pull_request = thread_context.pull_request.clone();
    if current_branch.is_some() {
        if let PullRequestLookup::Found(found) = lookup_pull_request(root).await {
            pull_request = Some(found);
        }
    }
    thread_context.pull_request = pull_request.clone();

    let (additions, deletions, changed_files, stats_source) = match pull_request.as_ref() {
        Some(pr) => (pr.additions, pr.deletions, pr.changed_files, "pull_request"),
        None => (
            local_stats.additions,
            local_stats.deletions,
            local_stats.changed_files,
            "branch",
        ),
    };
    let branch_changed = match (
        thread_context.branch.as_deref(),
        thread_context.base_branch.as_deref(),
    ) {
        (Some(branch), Some(base)) => branch != base,
        _ => false,
    };
    let view = GitContextView {
        branch: thread_context.branch.clone(),
        base_branch: thread_context.base_branch.clone(),
        branch_changed,
        additions,
        deletions,
        changed_files,
        stats_source: stats_source.to_string(),
        pull_request: pull_request.as_ref().map(PullRequestView::from),
    };
    GitContextInspection {
        thread_context,
        view,
    }
}

#[tauri::command]
async fn git_context(state: State<'_, AppState>) -> Result<GitContextView, String> {
    let snapshot = state
        .sessions
        .session_info_snapshot(|session| {
            (
                session.root.clone(),
                session.thread_id.clone(),
                session.thread.git_context.clone(),
            )
        })
        .map_err(map_session_err)?
        .ok_or_else(|| "no active session".to_string())?;
    let (root, thread_id, existing) = snapshot;
    let inspection = inspect_git_context(&root, existing.as_ref()).await;
    let next_context = inspection.thread_context.clone();
    match state
        .sessions
        .with_session_mut(|session| -> Result<(), String> {
            if session.thread_id != thread_id {
                return Ok(());
            }
            if session.thread.record_git_context(next_context) {
                let store = open_store(&session.root)?;
                // Update an existing chat, never create one. After deleting the
                // open chat the session holds a draft with no row and no git
                // context, and the front end asks for the context as soon as
                // the session changes — so this stamp was writing the draft
                // into history a second after the delete, which is the phantom
                // "Untitled chat" that appeared next to every chat the user
                // then created. The context still lands in memory either way,
                // and the first turn persists it with the message.
                if store.exists(&session.thread.id) {
                    store.save(&session.thread).map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        }) {
        Ok(result) => result?,
        // A turn owns the session body while it is streaming. Returning the
        // fresh computed view is safe; the next poll will persist it after the
        // turn releases the slot.
        Err(SessionError::Busy) => {}
        Err(error) => return Err(map_session_err(error)),
    }
    Ok(inspection.view)
}

fn workspace_review_without_git(repository: &str, summary: &str) -> WorkspaceReview {
    WorkspaceReview {
        summary: summary.to_string(),
        repository: repository.to_string(),
        changed_files: Vec::new(),
        changed_file_count: 0,
        patch_check: "unavailable".into(),
    }
}

fn changed_files_from_status(status: &str) -> Vec<String> {
    status
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.get(3..).unwrap_or(line).trim().to_string())
        .collect()
}

/// Run the smallest useful local review: inspect Git status and check the
/// patch for whitespace errors. It never runs project scripts or changes files.
async fn review_workspace_at(root: &Path) -> Result<WorkspaceReview, String> {
    let probe = match Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(root)
        .output()
        .await
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(workspace_review_without_git(
                "unavailable",
                "Git is not installed, so the workspace could not be reviewed.",
            ));
        }
        Err(error) => return Err(format!("could not inspect workspace: {error}")),
    };

    if !probe.status.success() || String::from_utf8_lossy(&probe.stdout).trim() != "true" {
        return Ok(workspace_review_without_git(
            "not_git",
            "This folder is not a Git repository.",
        ));
    }

    let status = Command::new("git")
        .args(["status", "--porcelain=v1", "--untracked-files=all"])
        .current_dir(root)
        .output()
        .await
        .map_err(|error| format!("could not read workspace changes: {error}"))?;
    if !status.status.success() {
        return Err("Git could not read the workspace changes.".into());
    }

    let all_changed_files = changed_files_from_status(&String::from_utf8_lossy(&status.stdout));
    let changed_file_count = all_changed_files.len();
    let changed_files = all_changed_files.into_iter().take(24).collect();

    let patch = Command::new("git")
        .args(["diff", "--check"])
        .current_dir(root)
        .output()
        .await
        .map_err(|error| format!("could not check the workspace patch: {error}"))?;
    let patch_check = if patch.status.success() {
        "clean"
    } else {
        "issues"
    };
    let summary = match (changed_file_count, patch_check) {
        (0, "clean") => "Working tree is clean.".to_string(),
        (0, _) => "The patch check found issues.".to_string(),
        (count, "clean") => format!(
            "{count} changed {}. No patch whitespace errors found.",
            if count == 1 { "file" } else { "files" }
        ),
        (count, _) => format!(
            "{count} changed {}. The patch check found issues.",
            if count == 1 { "file" } else { "files" }
        ),
    };

    Ok(WorkspaceReview {
        summary,
        repository: "git".into(),
        changed_files,
        changed_file_count,
        patch_check: patch_check.into(),
    })
}

#[tauri::command]
async fn verify_workspace(state: State<'_, AppState>) -> Result<WorkspaceReview, String> {
    let root = resolve_workspace_root(&state)?;
    review_workspace_at(&root).await
}

#[tauri::command]
fn list_delegation_jobs(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<DelegationJobView>, String> {
    let root = resolve_workspace_root(&state)?;
    let _ = state.delegations.reconcile(&app, &root)?;
    list_delegation_views(&root)
}

#[tauri::command]
fn get_delegation_job(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<DelegationJobView, String> {
    let root = resolve_workspace_root(&state)?;
    get_delegation_view(&root, &job_id)
}

#[tauri::command]
fn cancel_delegation_job(
    app: AppHandle,
    state: State<'_, AppState>,
    job_id: String,
) -> Result<DelegationJobView, String> {
    let root = resolve_workspace_root(&state)?;
    state.delegations.cancel(&app, &root, &job_id)
}

#[tauri::command]
fn retry_delegation_job(
    app: AppHandle,
    state: State<'_, AppState>,
    job_id: String,
) -> Result<DelegationJobView, String> {
    let root = resolve_workspace_root(&state)?;
    state.delegations.retry(&app, &root, &job_id)
}

#[tauri::command]
fn apply_delegation_job(
    app: AppHandle,
    state: State<'_, AppState>,
    job_id: String,
) -> Result<DelegationJobView, String> {
    let root = resolve_workspace_root(&state)?;
    state.delegations.apply(&app, &root, &job_id)
}

#[tauri::command]
fn context_usage(state: State<'_, AppState>) -> Result<ContextUsageView, String> {
    state
        .sessions
        .with_session_mut(|session| {
            estimate_context(&session.agent, session.thread.checkpoints.len())
        })
        .map_err(map_session_err)
}

/// Wire view for the UI. Avatar bytes live in `avatar.jpg`, not in JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserProfile {
    display_name: String,
    /// data:image/...;base64,... for display / optimized upload; empty clears file.
    #[serde(default)]
    avatar_data_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserProfileDisk {
    display_name: String,
}

/// Soft cap for optimized avatar payloads (JPEG ~128px is typically far smaller).
const MAX_AVATAR_DATA_URL_CHARS: usize = 80_000;
const MAX_AVATAR_BYTES: usize = 48_000;

fn user_profile_path() -> Result<PathBuf, String> {
    Ok(zest_config_dir()?.join("user-profile.json"))
}

fn user_avatar_path() -> Result<PathBuf, String> {
    Ok(zest_config_dir()?.join("avatar.jpg"))
}

fn load_avatar_data_url() -> Result<String, String> {
    let path = user_avatar_path()?;
    match std::fs::read(&path) {
        Ok(bytes) if !bytes.is_empty() => {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            Ok(format!("data:image/jpeg;base64,{b64}"))
        }
        Ok(_) => Ok(String::new()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.to_string()),
    }
}

fn write_avatar_from_data_url(data_url: &str) -> Result<(), String> {
    let path = user_avatar_path()?;
    let trimmed = data_url.trim();
    if trimmed.is_empty() {
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    if trimmed.chars().count() > MAX_AVATAR_DATA_URL_CHARS {
        return Err("avatar too large after optimize (pick a smaller image)".into());
    }
    let b64 = trimmed
        .split(',')
        .next_back()
        .ok_or_else(|| "invalid avatar data URL".to_string())?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("invalid avatar encoding: {e}"))?;
    if bytes.is_empty() {
        return Err("empty avatar".into());
    }
    if bytes.len() > MAX_AVATAR_BYTES {
        return Err("avatar too large after optimize (max ~48KB)".into());
    }
    if !bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Err("avatar must be JPEG (optimize in the UI before save)".into());
    }
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn get_user_profile() -> Result<UserProfile, String> {
    let path = user_profile_path()?;
    let display_name = match std::fs::read_to_string(&path) {
        Ok(raw) => {
            let disk: UserProfileDisk = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
            disk.display_name
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    Ok(UserProfile {
        display_name,
        avatar_data_url: load_avatar_data_url()?,
    })
}

#[tauri::command]
fn set_user_profile(profile: UserProfile) -> Result<UserProfile, String> {
    write_avatar_from_data_url(&profile.avatar_data_url)?;
    let path = user_profile_path()?;
    let disk = UserProfileDisk {
        display_name: profile.display_name.trim().to_string(),
    };
    let raw = serde_json::to_string_pretty(&disk).map_err(|e| e.to_string())?;
    std::fs::write(&path, raw).map_err(|e| e.to_string())?;
    Ok(UserProfile {
        display_name: disk.display_name,
        avatar_data_url: load_avatar_data_url()?,
    })
}

fn normalize_effort(effort: &str) -> String {
    zest_core::normalize_effort(effort)
}

/// User-facing turn errors. Connection refused to the local gateway is the
/// usual alpha failure mode and should not look like a missing system prompt.
fn format_turn_error(err: &HarnessError) -> String {
    if err.is_unreachable() {
        return "Zest could not reach the provider. Try reconnecting, then send your message again.".into();
    }
    if err.is_context_limit() {
        return "This conversation is too long for the selected model. Start a new conversation or shorten the request.".into();
    }
    if err.is_auth_problem() {
        return "This provider needs you to sign in again. Reconnect, then send your message again.".into();
    }
    "The provider could not complete the request. Try again.".into()
}

fn format_turn_error_for_provider(err: &HarnessError, provider_id: &str) -> String {
    if err.is_auth_problem() && !desktop_can_start_login(provider_id) {
        return match provider_id {
            "claude" => {
                "Claude Code could not authenticate this request. Sign in with the Claude Code CLI, then try again.".into()
            }
            "antigravity" => {
                "Gemini could not authenticate this request. Sign in with the Gemini CLI, then try again.".into()
            }
            _ => {
                "The provider could not authenticate this request. Check its API key or CLI sign-in, then try again.".into()
            }
        };
    }
    format_turn_error(err)
}

/// Wire label for approval / chat-event payloads (snake_case string).
fn tool_risk_wire(risk: ToolRisk) -> &'static str {
    match risk {
        ToolRisk::Read => "read",
        ToolRisk::Sensitive => "sensitive",
        ToolRisk::Write => "write",
        ToolRisk::Exec => "exec",
    }
}

fn provider_activity_status(status: &str) -> &'static str {
    match status.trim().to_ascii_lowercase().as_str() {
        "completed" | "complete" | "done" | "success" | "succeeded" => "done",
        "failed" | "failure" | "error" | "cancelled" | "canceled" => "error",
        _ => "running",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exhausted(inner: HarnessError) -> HarnessError {
        HarnessError::Exhausted {
            attempts: 3,
            source: Box::new(inner),
        }
    }

    /// The bug this guards: a gateway that was not running produced a Setup
    /// failure, and the picker told the user to Connect again — an OAuth flow
    /// that cannot start a process.
    #[test]
    fn a_setup_failure_never_asks_for_a_new_sign_in() {
        let failure = ProbeFailure::Setup("Zest could not start this provider. Try again.".into());
        assert!(!failure.needs_reconnect());
        assert_eq!(
            failure.user_message(),
            "Zest could not start this provider. Try again.",
            "a setup message is shown as written"
        );
    }

    #[test]
    fn a_cooled_down_session_still_asks_for_a_new_sign_in() {
        // Three failed attempts must not hide the auth envelope underneath.
        let failure = ProbeFailure::Turn(exhausted(HarnessError::Api {
            status: 503,
            body:
                r#"{"error":{"message":"auth_unavailable: no auth available (providers=claude)"}}"#
                    .into(),
        }));
        assert!(failure.needs_reconnect());
        let message = failure.user_message();
        assert_eq!(
            message,
            "This provider needs you to sign in again. Reconnect, then send your message again."
        );
    }

    #[test]
    fn auth_failures_offer_the_right_recovery_path() {
        let failure = HarnessError::Api {
            status: 401,
            body: r#"{"error":{"message":"authentication failed"}}"#.into(),
        };

        assert_eq!(
            format_turn_error_for_provider(&failure, "claude"),
            "This provider needs you to sign in again. Reconnect, then send your message again."
        );
        assert_eq!(
            format_turn_error_for_provider(&failure, "antigravity"),
            "Gemini could not authenticate this request. Sign in with the Gemini CLI, then try again."
        );
        assert_eq!(
            format_turn_error_for_provider(&failure, "codex"),
            "This provider needs you to sign in again. Reconnect, then send your message again."
        );
    }

    /// Opening a chat must not be gated on a credential check.
    ///
    /// `ensure_gateway_ready` can only ever fail with `Setup` — it does not build
    /// a registry or send a turn — so a cooled-down account cannot keep the chat
    /// from rendering. That is what moves the network round trip off the launch
    /// path; verification runs behind the UI and reports itself in a banner.
    #[test]
    fn opening_a_chat_cannot_fail_for_a_credential_reason() {
        // The only failure `ensure_gateway_ready` constructs.
        let blocked = ProbeFailure::Setup("Zest could not start this provider. Try again.".into());
        assert!(!blocked.needs_reconnect());

        // A native provider has no local gateway, so there is nothing to wait for
        // at all — the readiness half is a no-op and start is pure setup.
        let config = Config::parse(
            "[providers.anthropic]\nkind = \"anthropic\"\napi_key_env = \"ANTHROPIC_API_KEY\"\n",
        )
        .unwrap();
        assert_eq!(local_gateway_url(&config, "anthropic"), None);
    }

    #[test]
    fn an_overloaded_gateway_is_not_a_sign_in_problem() {
        let failure = ProbeFailure::Turn(exhausted(HarnessError::Api {
            status: 529,
            body: r#"{"error":{"message":"overloaded_error"}}"#.into(),
        }));
        assert!(!failure.needs_reconnect());
    }

    /// A native provider has no local process behind it, so there is nothing to
    /// start and nothing to blame for being down.
    #[test]
    fn only_gateway_providers_are_supervised() {
        let config = Config::parse(
            r#"
[providers.codex]
kind = "gateway"
base_url = "http://127.0.0.1:8317"
model = "gpt-5.6-sol"

[providers.anthropic]
kind = "anthropic"
api_key_env = "ANTHROPIC_API_KEY"
"#,
        )
        .unwrap();

        assert_eq!(
            local_gateway_url(&config, "codex").as_deref(),
            Some("http://127.0.0.1:8317")
        );
        assert_eq!(local_gateway_url(&config, "anthropic"), None);
        assert_eq!(local_gateway_url(&config, "missing"), None);
    }

    #[test]
    fn customized_external_worker_is_not_treated_as_a_preset() {
        let config = Config::parse(
            r#"
[agents.claude]
mode = "headless"
command = "claude"
args = ["--print", "custom-prompt-position"]
workspace = "isolated"
"#,
        )
        .unwrap();

        assert!(!external_agent_matches_preset(
            "claude",
            &config.agents["claude"]
        ));
    }

    #[test]
    fn previous_claude_presets_remain_toggleable() {
        let config = Config::parse(
            r#"
[agents.claude]
mode = "headless"
command = "claude"
args = ["--print", "--verbose", "--output-format", "stream-json", "--strict-mcp-config", "{prompt}"]
workspace = "isolated"
timeout_secs = 900
"#,
        )
        .unwrap();

        assert!(external_agent_matches_preset(
            "claude",
            &config.agents["claude"]
        ));
    }

    #[test]
    fn mcp_enabled_worker_remains_a_built_in_preset() {
        let preset = zest_core::config_edit::external_agent_preset_with_mcp("claude", true)
            .expect("Claude MCP preset");
        let raw = format!(
            "[agents.claude]\nmode = \"headless\"\ncommand = \"claude\"\nargs = {:?}\nallow_mcp = true\nworkspace = \"isolated\"\ntimeout_secs = 900\n",
            preset.args
        );
        let config = Config::parse(&raw).unwrap();

        assert!(external_agent_matches_preset(
            "claude",
            &config.agents["claude"]
        ));
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Err(err) = zest_core::ensure_user_config() {
        eprintln!("warning: could not create the user config: {err}");
    }
    zest_core::load_env();

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .manage(AppState {
            sessions: SessionController::new(),
            browser: Arc::new(BrowserHost::new()),
            login: Mutex::new(None),
            gateway: Mutex::new(None),
            persist: Mutex::new(HashMap::new()),
            // Validate the launch directory first so a project opened from a
            // terminal cannot be silently replaced by a stale remembered
            // workspace. Packaged launches fall through from their rejected
            // install directory to the remembered folder.
            workspace_root: Mutex::new(initial_workspace_root()),
            workspace_config: Mutex::new(None),
            policy: Arc::new(Mutex::new(ApprovalPolicy::new(DESKTOP_DEFAULT_MODE))),
            config_edit: Mutex::new(()),
            chat_summary_cache: Mutex::new(ChatSummaryCache::default()),
            space_state: Mutex::new(load_space_state()),
            delegations: Arc::new(DelegationCoordinator::new()),
        })
        .setup(|app| {
            app.state::<AppState>().browser.attach(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_providers,
            refresh_providers,
            list_external_agents,
            set_external_agent,
            set_external_agent_mcp,
            set_external_agent_model,
            check_external_agent,
            set_provider_key,
            delete_provider_key,
            provider_key_present,
            configure_api_provider,
            configure_anthropic_provider,
            configure_claude_code_provider,
            open_project_config,
            usage_snapshot,
            provider_quota,
            list_plugins,
            open_plugins_folder,
            set_plugin_enabled,
            now_playing,
            control_now_playing,
            set_now_playing_volume,
            usage_report,
            open_prices_file,
            refresh_rates,
            profile_stats,
            set_local_offset,
            last_provider,
            start_login,
            login_status,
            cancel_login,
            verify_provider,
            start_session,
            update_session_options,
            reset_session_options,
            list_threads,
            list_spaces,
            set_active_space,
            create_space,
            update_space,
            delete_space,
            move_project_to_space,
            forget_workspace,
            list_chat_projects,
            open_project_chat,
            load_thread,
            new_thread,
            fork_thread,
            fork_thread_from_checkpoint,
            rewind_thread,
            edit_message,
            compact_context,
            delete_thread,
            set_thread_pinned,
            rename_thread,
            send_message,
            save_markdown,
            cancel_turn,
            resolve_approval,
            resolve_question,
            generate_reading_diff,
            set_approval_mode,
            approval_mode,
            end_session,
            session_info,
            get_system_prompt,
            set_system_prompt,
            list_skills,
            list_commands,
            get_workspace_folder,
            list_workspace_files,
            read_workspace_file,
            pick_workspace_folder,
            pick_files,
            prepare_pasted_image,
            git_branch,
            git_context,
            workspace_changes,
            verify_workspace,
            context_usage,
            get_user_profile,
            set_user_profile,
            list_delegation_jobs,
            get_delegation_job,
            cancel_delegation_job,
            retry_delegation_job,
            apply_delegation_job
        ])
        .build(tauri::generate_context!())
        .expect("error while building Zest desktop")
        .run(|app_handle, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                app_handle.state::<AppState>().shutdown_gateway();
            }
        });
}

/// The bug these guard: a packaged install starts with its own program folder
/// as the working directory. First run adopted it as the project, session
/// startup then failed creating `<root>/.zest/threads`, and the picker showed
/// that as an unattributed error under the provider row — so a perfectly good
/// Codex sign-in looked broken.
#[cfg(test)]
mod workspace_root_tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zest-workspace-{name}-{}", new_id("test")));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writable_scratch_dir_is_usable() {
        let dir = scratch("writable");
        assert!(dir_is_writable(&dir));
        assert!(usable_workspace(dir).is_some());
    }

    #[test]
    fn missing_dir_is_not_a_workspace() {
        let missing = std::env::temp_dir().join(format!("zest-absent-{}", new_id("test")));
        assert!(usable_workspace(missing).is_none());
    }

    #[test]
    fn probe_leaves_nothing_behind() {
        let dir = scratch("no-litter");
        assert!(dir_is_writable(&dir));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    /// The exact first-run trap: the directory the executable lives in is
    /// rejected on location, so an elevated or writable install cannot quietly
    /// fill the program folder with chat history either.
    #[test]
    fn install_dir_is_never_a_workspace() {
        let Some(dir) = install_dir() else {
            return;
        };
        assert!(is_install_dir(&dir));
        assert!(usable_workspace(dir.clone()).is_none());
        // Subdirectories of the install belong to the install too.
        assert!(is_install_dir(&dir.join("resources")));
    }

    #[test]
    fn default_workspace_is_writable_when_available() {
        let Some(root) = default_workspace() else {
            return;
        };
        assert!(root.is_dir());
        assert!(dir_is_writable(&root));
        assert!(!is_install_dir(&root));
    }

    #[test]
    fn launch_directory_wins_over_a_stale_remembered_workspace() {
        let launch = PathBuf::from(r"D:\Code\Test");
        let remembered = PathBuf::from(r"D:\Code\zest");
        assert_eq!(
            choose_initial_workspace(Some(launch.clone()), Some(remembered), None),
            Some(launch)
        );
    }

    #[test]
    fn remembered_workspace_is_used_when_launch_directory_is_unusable() {
        let remembered = PathBuf::from(r"D:\Code\Test");
        let fallback = PathBuf::from(r"D:\Users\brite\Documents\Zest");
        assert_eq!(
            choose_initial_workspace(None, Some(remembered.clone()), Some(fallback)),
            Some(remembered)
        );
    }

    #[test]
    fn free_chat_does_not_reactivate_a_fallback_workspace() {
        let free = free_chats_root().unwrap();
        let stale = PathBuf::from(r"D:\Code\removed-project");

        assert_eq!(project_root_for_session(Some(&free), Some(stale)), None);
    }

    /// The UI keys the "choose another folder" guidance off this token, so the
    /// wording of the OS error must never be what decides whether it appears.
    #[test]
    fn workspace_failures_carry_the_ui_token() {
        assert!(no_writable_workspace_error().starts_with(WORKSPACE_NOT_WRITABLE));

        let unwritable = PathBuf::from(if cfg!(windows) {
            r"C:\Program Files\Zest\does-not-exist"
        } else {
            "/proc/zest-does-not-exist"
        });
        let message = workspace_write_error(&unwritable, "Access is denied. (os error 5)");
        assert!(message.starts_with(WORKSPACE_NOT_WRITABLE), "{message}");
    }

    /// A writable root means the failure was something else; do not mislabel it.
    #[test]
    fn writable_root_keeps_the_underlying_error() {
        let dir = scratch("real-error");
        let message = workspace_write_error(&dir, "disk quota exceeded");
        assert_eq!(message, "disk quota exceeded");
    }
}

#[cfg(all(test, feature = "export-bindings"))]
mod export_bindings {
    use super::*;

    #[test]
    fn export_bindings() {
        ChatEvent::export_all().expect("export ChatEvent bindings");
        SessionInfo::export_all().expect("export SessionInfo bindings");
        ProviderView::export_all().expect("export ProviderView bindings");
        ExternalAgentView::export_all().expect("export ExternalAgentView bindings");
        ExternalAgentCheckView::export_all().expect("export ExternalAgentCheckView bindings");
        ModelCapability::export_all().expect("export ModelCapability bindings");
        WorkspaceReview::export_all().expect("export WorkspaceReview bindings");
        WorkspaceFileChangeView::export_all().expect("export WorkspaceFileChange bindings");
        WorkspaceChangeView::export_all().expect("export WorkspaceChange bindings");
        PullRequestView::export_all().expect("export PullRequestView bindings");
        GitContextView::export_all().expect("export GitContext bindings");
        ThreadCheckpointView::export_all().expect("export ThreadCheckpoint bindings");
        TurnRecoveryView::export_all().expect("export TurnRecovery bindings");
        ToolMetaView::export_all().expect("export ToolMetaView bindings");
    }
}

#[cfg(test)]
mod characterization {
    use super::*;
    use zest_core::ToolRisk;

    #[test]
    fn git_numstat_counts_text_and_binary_files() {
        let stats = parse_git_numstat(b"3\t1\tsrc/lib.rs\n-\t-\tassets/logo.png\n");
        assert_eq!(stats.additions, 3);
        assert_eq!(stats.deletions, 1);
        assert_eq!(stats.changed_files, 2);
    }

    #[test]
    fn normalize_effort_aliases_and_default() {
        assert_eq!(normalize_effort("HIGH"), "high");
        assert_eq!(normalize_effort(" med "), "medium");
        assert_eq!(normalize_effort("extra_high"), "xhigh");
        assert_eq!(normalize_effort("nonsense"), "high");
        assert_eq!(normalize_effort("max"), "max");
    }

    #[test]
    fn tool_risk_wire_labels() {
        assert_eq!(tool_risk_wire(ToolRisk::Read), "read");
        assert_eq!(tool_risk_wire(ToolRisk::Sensitive), "sensitive");
        assert_eq!(tool_risk_wire(ToolRisk::Write), "write");
        assert_eq!(tool_risk_wire(ToolRisk::Exec), "exec");
    }

    #[test]
    fn chat_event_requires_identity_fields() {
        let event = ChatEvent::ToolCallResult {
            session_id: "s1".into(),
            thread_id: "th1".into(),
            turn_id: "turn-1".into(),
            message_id: "a1".into(),
            name: "write_file".into(),
            id: "t1".into(),
            summary: "wrote f.txt".into(),
            is_error: false,
            path: None,
            diff: None,
            metadata: None,
        };
        let v = serde_json::to_value(&event).unwrap();
        assert_eq!(v["kind"], "tool_call_result");
        assert_eq!(v["session_id"], "s1");
        assert_eq!(v["thread_id"], "th1");
        assert_eq!(v["turn_id"], "turn-1");
        assert_eq!(v["isError"], false);
    }

    #[test]
    fn apply_event_to_thread_covers_full_chat_sequence() {
        let mut thread = Thread::new();
        let sid = "s1";
        let tid = "th1";
        let turn = "turn-1";
        apply_event_to_thread(
            &mut thread,
            &ChatEvent::User {
                session_id: sid.into(),
                thread_id: tid.into(),
                turn_id: turn.into(),
                message_id: "u1".into(),
                text: "please edit".into(),
            },
        );
        apply_event_to_thread(
            &mut thread,
            &ChatEvent::AssistantStart {
                session_id: sid.into(),
                thread_id: tid.into(),
                turn_id: turn.into(),
                message_id: "a1".into(),
                command: None,
            },
        );
        apply_event_to_thread(
            &mut thread,
            &ChatEvent::TextDelta {
                session_id: sid.into(),
                thread_id: tid.into(),
                turn_id: turn.into(),
                message_id: "a1".into(),
                text: "ok".into(),
            },
        );
        apply_event_to_thread(
            &mut thread,
            &ChatEvent::Done {
                session_id: sid.into(),
                thread_id: tid.into(),
                turn_id: turn.into(),
                message_id: "a1".into(),
            },
        );
        assert_eq!(thread.messages.len(), 2);
    }

    #[test]
    fn workspace_review_parses_porcelain_paths_without_status_codes() {
        let files =
            changed_files_from_status(" M src/lib.rs\n?? notes/todo.md\nR  old.rs -> new.rs\n");
        assert_eq!(
            files,
            vec![
                "src/lib.rs".to_string(),
                "notes/todo.md".to_string(),
                "old.rs -> new.rs".to_string(),
            ]
        );
    }

    #[test]
    fn workspace_review_without_git_is_explicitly_unavailable() {
        let review = workspace_review_without_git("not_git", "not a repository");
        assert_eq!(review.repository, "not_git");
        assert_eq!(review.patch_check, "unavailable");
        assert_eq!(review.changed_file_count, 0);
        assert_eq!(review.summary, "not a repository");
    }

    fn slot(id: &'static str, status: AuthStatus) -> ProviderSlot {
        ProviderSlot {
            id,
            label: id,
            method: "test sign-in",
            status,
        }
    }

    fn config_with(ids: &[&str]) -> Config {
        let mut toml = String::new();
        for id in ids {
            toml.push_str(&format!(
                "[providers.{id}]\nkind = \"gateway\"\nbase_url = \"http://127.0.0.1:8317\"\nmodel = \"m\"\n\n"
            ));
        }
        Config::parse(&toml).expect("valid test config")
    }

    #[test]
    fn a_bare_workspace_inherits_the_active_provider_without_overwriting_it() {
        let mut destination = config_with(&["local"]);
        let cached = config_with(&["codex", "claude"]);
        merge_provider_tables(&mut destination, &cached, Some("codex"));

        assert!(destination.providers.contains_key("local"));
        assert!(destination.providers.contains_key("codex"));
        assert!(!destination.providers.contains_key("claude"));
    }

    /// A detected sign-in without provider configuration must still be setup-only.
    #[test]
    fn a_signed_in_provider_with_no_config_is_not_selectable() {
        let config = config_with(&["codex"]);
        let view = provider_view_from_slot(
            &slot("claude", AuthStatus::Ready { account: None }),
            &config,
        );

        assert!(!view.selectable, "must not be offered as usable");
        assert!(!view.configured);
        assert_eq!(view.status_kind, "unconfigured");
        assert_eq!(view.status_label, "Not configured");
        assert_eq!(
            view.detail,
            "Signed in. Configure this provider in Settings."
        );
        assert!(view.can_connect);
    }

    #[test]
    fn a_codex_sign_in_without_parent_config_is_not_launch_ready() {
        let config = Config::env_fallback();
        let view =
            provider_view_from_slot(&slot("codex", AuthStatus::Ready { account: None }), &config);

        assert!(!view.selectable);
        assert_eq!(view.status_kind, "unconfigured");
        assert_eq!(view.status_label, "Not configured");
        assert!(view.detail.contains("Configure this provider"));
    }

    #[test]
    fn desktop_exposes_claude_as_a_parent_login_choice() {
        assert_eq!(PICKER_IDS, &["codex", "claude"]);
        assert!(desktop_can_start_login("codex"));
        assert!(desktop_can_start_login("claude"));
        assert!(!desktop_can_start_login("antigravity"));
    }

    #[test]
    fn configured_direct_providers_are_visible_without_a_codex_parent() {
        let config = Config::parse(
            r#"
[providers.anthropic]
kind = "anthropic"
credential = "anthropic"

[providers.local]
kind = "openai_compatible"
base_url = "http://localhost:11434/v1"
model = "llama"

[providers.codex]
kind = "gateway"
base_url = "http://127.0.0.1:8317"
model = "gpt-5.6-sol"
"#,
        )
        .expect("valid direct-provider config");
        let mut rows = Vec::new();

        append_configured_direct_provider_views(&mut rows, &config);

        let ids: Vec<_> = rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, vec!["anthropic", "local"]);
        assert!(rows.iter().any(|row| row.id == "local" && row.selectable));
        assert!(rows.iter().any(|row| row.id == "anthropic"));
    }

    #[test]
    fn configured_provider_methods_match_the_secret_source() {
        assert_eq!(
            provider_method(&ProviderConfig::ClaudeCode {
                command: "claude".into(),
                model: Some("sonnet".into()),
                models: vec!["sonnet".into()],
                allow_mcp: false,
                permission_mode: zest_core::ClaudeCodePermissionMode::AcceptEdits,
                timeout_secs: 900,
            }),
            "Claude Code subscription"
        );
        assert_eq!(
            provider_method(&ProviderConfig::Anthropic {
                api_key_env: "ANTHROPIC_API_KEY".into(),
                model: None,
                credential: None,
            }),
            "Environment key"
        );
        assert_eq!(
            provider_method(&ProviderConfig::Anthropic {
                api_key_env: "ANTHROPIC_API_KEY".into(),
                model: Some("claude-opus-5".into()),
                credential: Some("anthropic".into()),
            }),
            "API key"
        );
        assert_eq!(
            provider_method(&ProviderConfig::OpenaiCompatible {
                base_url: "http://localhost:11434/v1".into(),
                model: "local".into(),
                models: vec![],
                efforts: vec![],
                credential: None,
                api_key_env: Some("LOCAL_API_KEY".into()),
            }),
            "Environment key"
        );
        assert_eq!(
            provider_method(&ProviderConfig::OpenaiCompatible {
                base_url: "http://localhost:11434/v1".into(),
                model: "local".into(),
                models: vec![],
                efforts: vec![],
                credential: None,
                api_key_env: None,
            }),
            "No authentication"
        );
    }

    #[test]
    fn a_signed_in_configured_provider_stays_selectable() {
        let config = config_with(&["codex", "claude"]);
        let view = provider_view_from_slot(
            &slot("claude", AuthStatus::Ready { account: None }),
            &config,
        );
        assert!(view.selectable);
        assert!(view.configured);
        assert_eq!(view.status_kind, "ready");
        assert_eq!(view.status_label, "Signed in");
    }

    #[test]
    fn a_configured_provider_without_a_sign_in_is_still_not_selectable() {
        // Both halves are required; config alone cannot serve a turn either.
        let config = config_with(&["claude"]);
        let view = provider_view_from_slot(
            &slot(
                "claude",
                AuthStatus::NotLoggedIn {
                    fix: "claude login".into(),
                },
            ),
            &config,
        );
        assert!(!view.selectable);
        assert!(view.configured);
    }

    #[tokio::test]
    async fn approval_hub_prepare_resolve_and_unknown_id() {
        let hub = ApprovalHub::new();
        hub.begin_turn("turn-1");
        hub.prepare("ap1");
        hub.resolve("ap1", ApprovalDecision::AllowOnce).unwrap();
        assert_eq!(hub.wait("ap1").await, ApprovalDecision::AllowOnce);

        assert!(hub.resolve("missing", ApprovalDecision::Deny).is_err());
        // Never prepared: no waiter, and the answer must be Deny, not a default
        // that happens to look permissive.
        assert_eq!(hub.wait("never-prepared").await, ApprovalDecision::Deny);

        hub.clear();
        assert!(hub.resolve("ap2", ApprovalDecision::AllowOnce).is_err());
    }

    #[tokio::test]
    async fn approval_hub_carries_a_session_grant_through() {
        // The three-way decision has to survive the channel — collapsing it to
        // a bool is what this widening exists to prevent.
        let hub = ApprovalHub::new();
        hub.begin_turn("turn-1");
        hub.prepare("ap-session");
        hub.resolve("ap-session", ApprovalDecision::AllowSession)
            .unwrap();
        assert_eq!(hub.wait("ap-session").await, ApprovalDecision::AllowSession);
    }

    #[tokio::test]
    async fn clearing_the_hub_denies_pending_waiters() {
        let hub = ApprovalHub::new();
        hub.begin_turn("turn-1");
        hub.prepare("ap-pending");
        hub.clear();
        assert_eq!(hub.wait("ap-pending").await, ApprovalDecision::Deny);
    }

    #[tokio::test]
    async fn question_hub_delivers_answers_and_dismisses_pending_questions() {
        let hub = QuestionHub::new();
        hub.begin_turn("turn-1");
        hub.prepare("q1");
        hub.resolve("q1", "Use React".into()).unwrap();
        assert_eq!(hub.wait("q1").await.unwrap(), "Use React");

        hub.prepare("q2");
        hub.clear();
        assert!(hub.wait("q2").await.is_err());
        assert!(hub.resolve("q3", "answer".into()).is_err());
    }
}
