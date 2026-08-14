//! Project-scoped chat thread projection.
//!
//! Threads live under `<workspace>/.zest/threads/<id>.json` so history follows the
//! repo you launched from. The projection is a durable UI transcript plus the
//! agent wire messages needed to restore model context on reopen.
//!
//! On-disk format is versioned ([`THREAD_FORMAT_VERSION`]) and binds provider /
//! wire-format metadata so reopen can migrate non-destructively.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::anthropic::types::Message;
use crate::error::{HarnessError, Result};
use crate::fsutil;

/// Current on-disk thread document version.
///
/// v2 adds optional typed [`ToolPart::metadata`] (delegation provenance).
/// v3 adds anchored checkpoint metadata while keeping every new field
/// optional so older thread files remain readable.
pub const THREAD_FORMAT_VERSION: u32 = 3;

/// Anthropic Messages API content blocks (today's only wire format).
pub const WIRE_FORMAT_ANTHROPIC_MESSAGES: &str = "anthropic_messages";

/// Maximum length for a user-supplied sidebar chat title.
pub const MAX_THREAD_TITLE_CHARS: usize = 200;

static ID_SEQ: AtomicU64 = AtomicU64::new(0);

/// Stable id for messages / turns / threads.
pub fn new_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = ID_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{nanos:x}-{seq:x}")
}

/// Validated thread identifier safe for use as a single path segment.
///
/// Rejects separators, absolute paths, drive prefixes, and `.` / `..` segments
/// so store paths cannot escape `.zest/threads/`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ThreadId(String);

impl ThreadId {
    pub fn parse(raw: impl AsRef<str>) -> std::result::Result<Self, String> {
        let s = raw.as_ref();
        validate_thread_id(s)?;
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ThreadId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn validate_thread_id(s: &str) -> std::result::Result<(), String> {
    if s.is_empty() {
        return Err("thread id must not be empty".into());
    }
    if s.len() > 200 {
        return Err("thread id is too long".into());
    }
    if s.contains('/') || s.contains('\\') {
        return Err("thread id must not contain path separators".into());
    }
    if s.contains('\0') {
        return Err("thread id must not contain NUL".into());
    }
    // Drive prefix (`C:`) or bare colon tricks.
    if s.len() >= 2 && s.as_bytes()[1] == b':' {
        return Err("thread id must not contain a drive prefix".into());
    }
    if s.contains(':') {
        return Err("thread id must not contain ':'".into());
    }
    // Dot segments (whole id or as a path component if separators slipped through).
    if s == "." || s == ".." {
        return Err("thread id must not be a dot segment".into());
    }
    if s.split(['/', '\\']).any(|part| part == "." || part == "..") {
        return Err("thread id must not contain dot segments".into());
    }
    // Keep store filenames boring: alnum, hyphen, underscore only.
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("thread id may only contain ASCII letters, digits, '-' and '_'".into());
    }
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPart {
    pub id: String,
    pub name: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    /// Typed side-channel (e.g. delegation provenance). Empty on v1 threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<crate::tools::ToolMetadata>,
}

impl ToolPart {
    pub fn running(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            status: "running".into(),
            summary: None,
            approval_id: None,
            path: None,
            diff: None,
            metadata: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum StoredMessage {
    User {
        id: String,
        text: String,
    },
    Assistant {
        id: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        thinking: String,
        #[serde(default)]
        tools: Vec<ToolPart>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// Slash command that produced this turn. Persisted so a reopened chat
        /// still frames the answer the way it was framed when written —
        /// otherwise an old plan silently degrades to plain text and looks
        /// like a rendering bug. Optional, so older threads load unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
        #[serde(default)]
        streaming: bool,
    },
}

impl StoredMessage {
    pub fn id(&self) -> &str {
        match self {
            Self::User { id, .. } | Self::Assistant { id, .. } => id,
        }
    }
}

#[allow(clippy::type_complexity)]
fn assistant_fields(
    msg: &mut StoredMessage,
) -> Option<(
    &mut String,
    &mut String,
    &mut Vec<ToolPart>,
    &mut Option<String>,
    &mut bool,
)> {
    match msg {
        StoredMessage::Assistant {
            text,
            thinking,
            tools,
            error,
            streaming,
            ..
        } => Some((text, thinking, tools, error, streaming)),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestLink {
    /// Repository identity is optional because the URL is authoritative and
    /// GitHub CLI can return a PR even when the remote is not named `origin`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub is_draft: bool,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGitContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<PullRequestLink>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummary {
    pub id: String,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    pub message_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_context: Option<ThreadGitContext>,
}

/// The reason a checkpoint exists. The UI uses this to give turn checkpoints
/// and maintenance checkpoints slightly different affordances without
/// inspecting labels written by a caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadCheckpointKind {
    Turn,
    Compaction,
    Manual,
}

impl Default for ThreadCheckpointKind {
    fn default() -> Self {
        Self::Turn
    }
}

/// A durable conversation checkpoint. The full snapshot lives beside the
/// thread file; this small record is what the UI needs to render the rewind
/// affordance without loading every snapshot up front.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadCheckpoint {
    pub id: String,
    pub created_at: u64,
    pub label: String,
    pub message_count: usize,
    pub agent_message_count: usize,
    /// The user message that follows this checkpoint when it was created.
    /// Older checkpoints fall back to `message_count` as their anchor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_message_id: Option<String>,
    /// Short local preview used by the checkpoint rail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(default)]
    pub kind: ThreadCheckpointKind,
}

/// Typed outcomes when loading a thread from disk.
#[derive(Debug, Error)]
pub enum ThreadLoadError {
    #[error("thread `{0}` not found")]
    Missing(String),
    #[error("thread `{id}` is corrupt: {detail}")]
    Corrupt { id: String, detail: String },
    #[error(
        "thread `{id}` format v{found} is newer than supported v{supported}; refusing to rewrite"
    )]
    UnsupportedVersion {
        id: String,
        found: u32,
        supported: u32,
    },
    #[error("thread `{id}` I/O error: {detail}")]
    Io { id: String, detail: String },
    #[error("invalid thread id: {0}")]
    InvalidId(String),
    #[error("thread `{id}` belongs to provider `{owned}`, not `{wanted}`")]
    ProviderMismatch {
        id: String,
        owned: String,
        wanted: String,
    },
}

impl From<ThreadLoadError> for HarnessError {
    fn from(err: ThreadLoadError) -> Self {
        HarnessError::Other(err.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    /// On-disk schema version. Missing / 0 in pre-alpha files → migrated to 1.
    #[serde(default)]
    pub version: u32,
    pub id: String,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// User-controlled sidebar pin. Missing in older thread files means false.
    #[serde(default)]
    pub pinned: bool,
    /// Provider that owns this conversation (parent is always pinned).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// Provider-owned continuation cursor. It is non-secret and optional so
    /// gateway, Anthropic, Claude Code, and legacy v2 threads remain unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session: Option<crate::provider::ProviderSessionRef>,
    /// Git checkout and optional pull request associated with this chat.
    ///
    /// This is deliberately optional so older `.zest` thread files remain
    /// readable and do not acquire repository metadata until the chat is
    /// opened in a Git workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_context: Option<ThreadGitContext>,
    /// Wire format for `agent_messages` (e.g. anthropic_messages).
    #[serde(default = "default_wire_format")]
    pub wire_format: String,
    #[serde(default)]
    pub messages: Vec<StoredMessage>,
    /// Conversation checkpoints stored under `.zest/threads/checkpoints/`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checkpoints: Vec<ThreadCheckpoint>,
    /// Wire messages for restoring `Agent.messages` so the model sees prior context.
    #[serde(default)]
    pub agent_messages: Vec<Message>,
}

fn default_wire_format() -> String {
    WIRE_FORMAT_ANTHROPIC_MESSAGES.to_string()
}

/// Outcome of loading a thread, including non-fatal migration notes.
#[derive(Debug, Clone)]
pub struct ThreadLoad {
    pub thread: Thread,
    /// Soft warning for the UI (migration notes, recovered interrupted tools, …).
    pub warning: Option<String>,
}

impl Thread {
    pub fn new() -> Self {
        let now = now_secs();
        let id = ThreadId::parse(new_id("thread")).expect("generated thread id is always valid");
        Self {
            version: THREAD_FORMAT_VERSION,
            id: id.as_str().to_string(),
            created_at: now,
            updated_at: now,
            title: None,
            pinned: false,
            provider_id: None,
            provider_session: None,
            git_context: None,
            wire_format: default_wire_format(),
            messages: Vec::new(),
            checkpoints: Vec::new(),
            agent_messages: Vec::new(),
        }
    }

    pub fn with_provider(mut self, provider_id: impl Into<String>) -> Self {
        self.provider_id = Some(provider_id.into());
        self
    }

    /// Set the sidebar pin without changing activity order. Pinning is a
    /// navigation preference, not conversation activity.
    pub fn set_pinned(&mut self, pinned: bool) -> bool {
        if self.pinned == pinned {
            return false;
        }
        self.pinned = pinned;
        true
    }

    /// Replace the user-visible sidebar title without changing activity order.
    pub fn set_title(&mut self, title: &str) -> std::result::Result<bool, String> {
        let title = title.trim();
        if title.is_empty() {
            return Err("chat title must not be empty".into());
        }
        if title.chars().count() > MAX_THREAD_TITLE_CHARS {
            return Err(format!(
                "chat title must be {MAX_THREAD_TITLE_CHARS} characters or fewer"
            ));
        }
        if title.contains('\0') {
            return Err("chat title must not contain NUL".into());
        }
        if self.title.as_deref() == Some(title) {
            return Ok(false);
        }
        self.title = Some(title.to_string());
        Ok(true)
    }

    /// Fill missing version / wire-format fields from older files.
    ///
    /// Returns `Err` when the on-disk version is newer than this binary supports
    /// — callers must not rewrite those threads.
    pub fn migrate_in_place(&mut self) -> std::result::Result<Option<String>, ThreadLoadError> {
        if self.version > THREAD_FORMAT_VERSION {
            return Err(ThreadLoadError::UnsupportedVersion {
                id: self.id.clone(),
                found: self.version,
                supported: THREAD_FORMAT_VERSION,
            });
        }
        let mut notes = Vec::new();
        if self.version < THREAD_FORMAT_VERSION {
            let from = self.version;
            self.version = THREAD_FORMAT_VERSION;
            if from == 0 {
                notes.push(format!(
                    "migrated thread to format v{THREAD_FORMAT_VERSION}"
                ));
            } else {
                notes.push(format!(
                    "migrated thread from format v{from} to v{THREAD_FORMAT_VERSION}"
                ));
            }
        }
        if self.wire_format.trim().is_empty() {
            self.wire_format = default_wire_format();
            notes.push("filled missing wireFormat".into());
        }
        Ok(if notes.is_empty() {
            None
        } else {
            Some(notes.join("; "))
        })
    }

    /// Refuse to change an already-pinned provider owner.
    pub fn assert_provider(&self, provider_id: &str) -> std::result::Result<(), ThreadLoadError> {
        match self.provider_id.as_deref() {
            None => Ok(()),
            Some(owned) if owned == provider_id => Ok(()),
            Some(owned) => Err(ThreadLoadError::ProviderMismatch {
                id: self.id.clone(),
                owned: owned.to_string(),
                wanted: provider_id.to_string(),
            }),
        }
    }

    /// Pin provider once. Never rewrites an existing owner.
    pub fn ensure_provider(
        &mut self,
        provider_id: &str,
    ) -> std::result::Result<(), ThreadLoadError> {
        self.assert_provider(provider_id)?;
        if self.provider_id.is_none() {
            self.provider_id = Some(provider_id.to_string());
        }
        Ok(())
    }

    /// Capture the checkout from which this chat started, without overwriting
    /// a context already recorded for the thread.
    pub fn ensure_git_context(
        &mut self,
        base_branch: Option<String>,
        start_commit: Option<String>,
    ) -> bool {
        if base_branch.is_none() && start_commit.is_none() {
            return false;
        }
        let context = self
            .git_context
            .get_or_insert_with(ThreadGitContext::default);
        let mut changed = false;
        if context.base_branch.is_none() && base_branch.is_some() {
            context.base_branch = base_branch;
            changed = true;
        }
        if context.start_commit.is_none() && start_commit.is_some() {
            context.start_commit = start_commit;
            changed = true;
        }
        if changed {
            self.touch();
        }
        changed
    }

    /// Record the checkout currently visible while the chat is open.
    pub fn record_git_branch(&mut self, branch: Option<String>) -> bool {
        let Some(branch) = branch else {
            return false;
        };
        let context = self
            .git_context
            .get_or_insert_with(ThreadGitContext::default);
        if context.branch.as_deref() == Some(branch.as_str()) {
            return false;
        }
        context.branch = Some(branch);
        self.touch();
        true
    }

    /// Replace the latest Git snapshot while preserving the conversation’s
    /// durable association with the checkout and pull request.
    pub fn record_git_context(&mut self, context: ThreadGitContext) -> bool {
        if self.git_context.as_ref() == Some(&context) {
            return false;
        }
        self.git_context = Some(context);
        self.touch();
        true
    }

    /// Convert interrupted approvals / still-running tools into terminal error
    /// cards so a restart never leaves forever-pending UI state.
    pub fn terminalize_interrupted(&mut self) -> bool {
        let mut changed = false;
        for msg in &mut self.messages {
            let Some((_, _, tools, _, streaming)) = assistant_fields(msg) else {
                continue;
            };
            if *streaming {
                *streaming = false;
                changed = true;
            }
            for tool in tools.iter_mut() {
                match tool.status.as_str() {
                    "awaiting_approval" => {
                        tool.status = "error".into();
                        tool.summary = Some(match tool.summary.take() {
                            Some(s) if !s.is_empty() => format!("{s} (approval interrupted)"),
                            _ => "approval interrupted".into(),
                        });
                        tool.approval_id = None;
                        changed = true;
                    }
                    "running" => {
                        tool.status = "error".into();
                        tool.summary = Some(match tool.summary.take() {
                            Some(s) if !s.is_empty() => format!("{s} (interrupted)"),
                            _ => "tool interrupted".into(),
                        });
                        tool.approval_id = None;
                        changed = true;
                    }
                    _ => {}
                }
            }
        }
        if changed {
            self.touch();
        }
        changed
    }

    pub fn thread_id(&self) -> std::result::Result<ThreadId, String> {
        ThreadId::parse(&self.id)
    }

    pub fn summary(&self) -> ThreadSummary {
        ThreadSummary {
            id: self.id.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            title: self.title.clone(),
            pinned: self.pinned,
            provider_id: self.provider_id.clone(),
            message_count: self.messages.len(),
            git_context: self.git_context.clone(),
        }
    }

    pub fn touch(&mut self) {
        self.updated_at = now_secs();
    }

    fn ensure_title_from_user(&mut self, text: &str) {
        if self.title.is_some() {
            return;
        }
        let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if flat.is_empty() {
            return;
        }
        let title: String = flat.chars().take(72).collect();
        self.title = Some(title);
    }

    fn find_mut(&mut self, id: &str) -> Option<&mut StoredMessage> {
        self.messages.iter_mut().find(|m| m.id() == id)
    }

    fn ensure_assistant(&mut self, message_id: &str) {
        if self.find_mut(message_id).is_some() {
            return;
        }
        self.messages.push(StoredMessage::Assistant {
            id: message_id.to_string(),
            text: String::new(),
            thinking: String::new(),
            tools: Vec::new(),
            error: None,
            command: None,
            streaming: true,
        });
    }

    /// Upsert UI projection from a chat-event shape (desktop emits these).
    pub fn apply_user(&mut self, message_id: &str, text: &str) {
        if self.find_mut(message_id).is_none() {
            self.messages.push(StoredMessage::User {
                id: message_id.to_string(),
                text: text.to_string(),
            });
        }
        self.ensure_title_from_user(text);
        self.touch();
    }

    /// Create an empty streaming assistant row before the first delta.
    pub fn apply_assistant_start(&mut self, message_id: &str, command: Option<&str>) {
        self.ensure_assistant(message_id);
        if let Some(name) = command {
            if let Some(StoredMessage::Assistant { command, .. }) = self.find_mut(message_id) {
                *command = Some(name.to_string());
            }
        }
        self.touch();
    }

    pub fn apply_text_delta(&mut self, message_id: &str, text: &str) {
        self.ensure_assistant(message_id);
        if let Some(msg) = self.find_mut(message_id) {
            if let Some((body, _, _, _, streaming)) = assistant_fields(msg) {
                body.push_str(text);
                *streaming = true;
            }
        }
        self.touch();
    }

    pub fn apply_thinking_delta(&mut self, message_id: &str, text: &str) {
        self.ensure_assistant(message_id);
        if let Some(msg) = self.find_mut(message_id) {
            if let Some((_, thinking, _, _, streaming)) = assistant_fields(msg) {
                thinking.push_str(text);
                *streaming = true;
            }
        }
        self.touch();
    }

    pub fn apply_tool_start(&mut self, message_id: &str, tool_id: &str, name: &str) {
        self.ensure_assistant(message_id);
        if let Some(msg) = self.find_mut(message_id) {
            if let Some((_, _, tools, _, streaming)) = assistant_fields(msg) {
                if !tools.iter().any(|t| t.id == tool_id) {
                    tools.push(ToolPart::running(tool_id, name));
                }
                *streaming = true;
            }
        }
        self.touch();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn apply_approval_needed(
        &mut self,
        message_id: &str,
        tool_call_id: &str,
        tool_name: &str,
        approval_id: &str,
        path: &str,
        summary: &str,
        diff: &str,
    ) {
        self.ensure_assistant(message_id);
        if let Some(msg) = self.find_mut(message_id) {
            if let Some((_, _, tools, _, streaming)) = assistant_fields(msg) {
                if let Some(tool) = tools.iter_mut().find(|t| t.id == tool_call_id) {
                    tool.status = "awaiting_approval".into();
                    tool.approval_id = Some(approval_id.to_string());
                    tool.path = Some(path.to_string());
                    tool.summary = Some(summary.to_string());
                    tool.diff = Some(diff.to_string());
                } else {
                    tools.push(ToolPart {
                        id: tool_call_id.to_string(),
                        name: tool_name.to_string(),
                        status: "awaiting_approval".into(),
                        summary: Some(summary.to_string()),
                        approval_id: Some(approval_id.to_string()),
                        path: Some(path.to_string()),
                        diff: Some(diff.to_string()),
                        metadata: None,
                    });
                }
                *streaming = true;
            }
        }
        self.touch();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn apply_tool_result(
        &mut self,
        message_id: &str,
        tool_id: &str,
        name: &str,
        summary: &str,
        is_error: bool,
        path: Option<&str>,
        diff: Option<&str>,
        metadata: Option<crate::tools::ToolMetadata>,
    ) {
        self.ensure_assistant(message_id);
        if let Some(msg) = self.find_mut(message_id) {
            if let Some((_, _, tools, _, streaming)) = assistant_fields(msg) {
                if let Some(tool) = tools.iter_mut().find(|t| t.id == tool_id) {
                    tool.status = if is_error { "error" } else { "done" }.into();
                    tool.summary = Some(summary.to_string());
                    tool.approval_id = None;
                    if let Some(path) = path {
                        tool.path = Some(path.to_string());
                    }
                    if let Some(diff) = diff {
                        tool.diff = Some(diff.to_string());
                    }
                    if metadata.is_some() {
                        tool.metadata = metadata;
                    }
                    // Keep path/diff on the card for context after allow/deny.
                } else {
                    tools.push(ToolPart {
                        id: tool_id.to_string(),
                        name: name.to_string(),
                        status: if is_error { "error" } else { "done" }.into(),
                        summary: Some(summary.to_string()),
                        approval_id: None,
                        path: path.map(str::to_string),
                        diff: diff.map(str::to_string),
                        metadata,
                    });
                }
                *streaming = true;
            }
        }
        self.touch();
    }

    pub fn apply_done(&mut self, message_id: &str) {
        if let Some(msg) = self.find_mut(message_id) {
            if let Some((_, _, _, _, streaming)) = assistant_fields(msg) {
                *streaming = false;
            }
        }
        self.touch();
    }

    pub fn apply_error(&mut self, message_id: &str, message: &str) {
        self.ensure_assistant(message_id);
        if let Some(msg) = self.find_mut(message_id) {
            if let Some((_, _, _, error, streaming)) = assistant_fields(msg) {
                *error = Some(message.to_string());
                *streaming = false;
            }
        }
        self.touch();
    }

    pub fn set_agent_messages(&mut self, messages: Vec<Message>) {
        self.agent_messages = messages;
        self.touch();
    }
}

impl Default for Thread {
    fn default() -> Self {
        Self::new()
    }
}

/// `<workspace>/.zest/threads`.
pub struct ThreadStore {
    dir: PathBuf,
}

impl ThreadStore {
    pub fn open(workspace_root: impl AsRef<Path>) -> Result<Self> {
        let dir = workspace_root.as_ref().join(".zest").join("threads");
        fs::create_dir_all(&dir).map_err(|e| {
            HarnessError::Other(format!("create thread dir {}: {e}", dir.display()))
        })?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path_for(&self, id: &ThreadId) -> PathBuf {
        self.dir.join(format!("{}.json", id.as_str()))
    }

    fn checkpoints_dir_for(&self, id: &ThreadId) -> PathBuf {
        self.dir.join("checkpoints").join(id.as_str())
    }

    pub fn save(&self, thread: &Thread) -> Result<()> {
        let id = ThreadId::parse(&thread.id)
            .map_err(|e| HarnessError::Other(format!("invalid thread id: {e}")))?;
        if thread.version > THREAD_FORMAT_VERSION {
            return Err(ThreadLoadError::UnsupportedVersion {
                id: thread.id.clone(),
                found: thread.version,
                supported: THREAD_FORMAT_VERSION,
            }
            .into());
        }
        let path = self.path_for(&id);

        // Normalisation used to clone the whole thread unconditionally, to set
        // two fields that are almost always already correct. On a long
        // conversation that is a deep copy of every message on every save, and
        // saves happen several times a second while a turn streams. Clone only
        // when there is actually something to fix.
        let needs_normalising =
            thread.version != THREAD_FORMAT_VERSION || thread.wire_format.trim().is_empty();
        let result = if needs_normalising {
            let mut normalised = thread.clone();
            normalised.version = THREAD_FORMAT_VERSION;
            if normalised.wire_format.trim().is_empty() {
                normalised.wire_format = default_wire_format();
            }
            fsutil::atomic_write_json(&path, &normalised)
        } else {
            fsutil::atomic_write_json(&path, thread)
        };

        result.map_err(|e| HarnessError::Other(format!("write thread {}: {e}", path.display())))?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> Result<Thread> {
        Ok(self.load_with_recovery(id)?.thread)
    }

    /// Load + migrate + terminalize interrupted in-flight tool/approval cards.
    pub fn load_with_recovery(&self, id: &str) -> Result<ThreadLoad> {
        self.load_typed(id).map_err(Into::into)
    }

    /// Typed load used by desktop restore / provider ownership checks.
    pub fn load_typed(&self, id: &str) -> std::result::Result<ThreadLoad, ThreadLoadError> {
        let tid = ThreadId::parse(id).map_err(ThreadLoadError::InvalidId)?;
        let path = self.path_for(&tid);
        let body = match fs::read_to_string(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ThreadLoadError::Missing(tid.as_str().to_string()));
            }
            Err(e) => {
                return Err(ThreadLoadError::Io {
                    id: tid.as_str().to_string(),
                    detail: e.to_string(),
                });
            }
        };
        let mut thread: Thread = match serde_json::from_str(&body) {
            Ok(t) => t,
            Err(e) => {
                let preserved = preserve_corrupt(&path).map_err(|err| ThreadLoadError::Io {
                    id: tid.as_str().to_string(),
                    detail: err.to_string(),
                })?;
                return Err(ThreadLoadError::Corrupt {
                    id: tid.as_str().to_string(),
                    detail: format!("preserved as {}; parse error: {e}", preserved.display()),
                });
            }
        };

        // Ensure id in file matches request (path is authoritative).
        if thread.id != tid.as_str() {
            thread.id = tid.as_str().to_string();
        }

        let mut warnings = Vec::new();
        match thread.migrate_in_place() {
            Ok(Some(note)) => warnings.push(note),
            Ok(None) => {}
            Err(e) => return Err(e),
        }
        if thread.terminalize_interrupted() {
            warnings.push("interrupted tools/approvals were closed after restart".into());
            // Persist the terminalized projection so reopen stays stable.
            let _ = self.save(&thread);
        } else if !warnings.is_empty() {
            let _ = self.save(&thread);
        }

        Ok(ThreadLoad {
            thread,
            warning: if warnings.is_empty() {
                None
            } else {
                Some(warnings.join("; "))
            },
        })
    }

    /// Load and reject cross-provider restore (never rewrites `provider_id`).
    pub fn load_for_provider(
        &self,
        id: &str,
        provider_id: &str,
    ) -> std::result::Result<ThreadLoad, ThreadLoadError> {
        let loaded = self.load_typed(id)?;
        loaded.thread.assert_provider(provider_id)?;
        Ok(loaded)
    }

    pub fn load_or_none(&self, id: &str) -> Option<Thread> {
        self.load_with_recovery(id).ok().map(|l| l.thread)
    }

    pub fn create(&self) -> Result<Thread> {
        let thread = Thread::new();
        self.save(&thread)?;
        Ok(thread)
    }

    pub fn create_for_provider(&self, provider_id: &str) -> Result<Thread> {
        let thread = Thread::new().with_provider(provider_id);
        self.save(&thread)?;
        Ok(thread)
    }

    /// Save a full conversation snapshot before a turn mutates the thread.
    ///
    /// The snapshot is intentionally separate from the main thread document:
    /// retaining a bounded list of metadata there keeps the sidebar cheap while
    /// rewind still has the exact provider wire history it needs.
    pub fn create_checkpoint(
        &self,
        thread: &mut Thread,
        label: impl Into<String>,
    ) -> Result<ThreadCheckpoint> {
        self.create_checkpoint_with_metadata(thread, label, None, None, ThreadCheckpointKind::Turn)
    }

    /// Save a checkpoint with the metadata needed by the desktop's timeline
    /// rail. The snapshot stays the same durable source of truth; the extra
    /// fields are only a cheap navigation index.
    pub fn create_checkpoint_with_metadata(
        &self,
        thread: &mut Thread,
        label: impl Into<String>,
        anchor_message_id: Option<String>,
        preview: Option<String>,
        kind: ThreadCheckpointKind,
    ) -> Result<ThreadCheckpoint> {
        let thread_id = ThreadId::parse(&thread.id)
            .map_err(|e| HarnessError::Other(format!("invalid thread id: {e}")))?;
        let checkpoint_id =
            ThreadId::parse(new_id("checkpoint")).expect("generated checkpoint id is always valid");
        let dir = self.checkpoints_dir_for(&thread_id);
        fs::create_dir_all(&dir).map_err(|e| {
            HarnessError::Other(format!("create checkpoint dir {}: {e}", dir.display()))
        })?;

        let mut snapshot = thread.clone();
        // Metadata belongs to the live thread. Keeping it out of the snapshot
        // prevents a rewind from recursively carrying future checkpoints back.
        snapshot.checkpoints.clear();
        let path = dir.join(format!("{}.json", checkpoint_id.as_str()));
        fsutil::atomic_write_json(&path, &snapshot).map_err(|e| {
            HarnessError::Other(format!("write checkpoint {}: {e}", path.display()))
        })?;

        let checkpoint = ThreadCheckpoint {
            id: checkpoint_id.to_string(),
            created_at: now_secs(),
            label: label.into(),
            message_count: thread.messages.len(),
            agent_message_count: thread.agent_messages.len(),
            anchor_message_id,
            preview,
            kind,
        };
        thread.checkpoints.push(checkpoint.clone());

        Self::prune_checkpoints(thread, &dir);
        thread.touch();
        self.save(thread)?;
        Ok(checkpoint)
    }

    /// Ceiling on how many snapshots one thread keeps.
    ///
    /// Enough to rewind through a long working session.
    const MAX_CHECKPOINTS: usize = 24;

    /// Ceiling on what those snapshots may occupy on disk.
    ///
    /// A count alone is not a bound. Every checkpoint is a complete copy of the
    /// conversation, so on a long thread twenty-four of them are twenty-four
    /// full transcripts — the limit that binds is bytes, and it is the one that
    /// was missing. Old snapshots go first, because the useful ones are the
    /// recent ones.
    const MAX_CHECKPOINT_BYTES: u64 = 64 * 1024 * 1024;

    /// Drop the oldest snapshots until both ceilings are satisfied.
    ///
    /// Always leaves one behind. A thread with a single enormous checkpoint has
    /// nothing useful to delete — removing it would buy space by making rewind
    /// impossible, which is the one thing checkpoints exist for.
    fn prune_checkpoints(thread: &mut Thread, dir: &Path) {
        let size_of = |id: &str| -> u64 {
            fs::metadata(dir.join(format!("{id}.json")))
                .map(|meta| meta.len())
                .unwrap_or(0)
        };

        let mut total: u64 = thread
            .checkpoints
            .iter()
            .map(|checkpoint| size_of(&checkpoint.id))
            .sum();

        while thread.checkpoints.len() > 1
            && (thread.checkpoints.len() > Self::MAX_CHECKPOINTS
                || total > Self::MAX_CHECKPOINT_BYTES)
        {
            let oldest = thread.checkpoints.remove(0);
            total = total.saturating_sub(size_of(&oldest.id));
            let _ = fs::remove_file(dir.join(format!("{}.json", oldest.id)));
        }
    }

    /// Restore a checkpoint snapshot. The caller decides whether it should
    /// replace the active session; this method only validates and reads it.
    pub fn load_checkpoint(&self, thread_id: &str, checkpoint_id: &str) -> Result<Thread> {
        let thread_id = ThreadId::parse(thread_id)
            .map_err(|e| HarnessError::Other(format!("invalid thread id: {e}")))?;
        let checkpoint_id = ThreadId::parse(checkpoint_id)
            .map_err(|e| HarnessError::Other(format!("invalid checkpoint id: {e}")))?;
        let path = self
            .checkpoints_dir_for(&thread_id)
            .join(format!("{}.json", checkpoint_id.as_str()));
        let raw = fs::read_to_string(&path)
            .map_err(|e| HarnessError::Other(format!("read checkpoint {}: {e}", path.display())))?;
        let mut snapshot: Thread = serde_json::from_str(&raw).map_err(|e| {
            HarnessError::Other(format!("checkpoint {} is corrupt: {e}", path.display()))
        })?;
        if snapshot.id != thread_id.as_str() {
            return Err(HarnessError::Other(
                "checkpoint belongs to a different thread".into(),
            ));
        }
        snapshot.checkpoints.clear();
        Ok(snapshot)
    }

    /// Restore a checkpoint and remove all newer checkpoint snapshots.
    ///
    /// Conversation rewind is intentionally independent of the workspace: the
    /// thread store only changes transcript snapshots and provider cursors.
    pub fn rewind_to_checkpoint(&self, thread: &Thread, checkpoint_id: &str) -> Result<Thread> {
        let checkpoint_index = thread
            .checkpoints
            .iter()
            .position(|checkpoint| checkpoint.id == checkpoint_id)
            .ok_or_else(|| {
                HarnessError::Other("checkpoint is not part of this conversation".into())
            })?;
        let mut restored = self.load_checkpoint(&thread.id, checkpoint_id)?;
        let discarded_checkpoints = thread
            .checkpoints
            .iter()
            .skip(checkpoint_index + 1)
            .map(|checkpoint| checkpoint.id.clone())
            .collect::<Vec<_>>();
        restored.checkpoints = thread.checkpoints[..=checkpoint_index].to_vec();
        restored.provider_session = None;
        restored.touch();
        self.save(&restored)?;

        let thread_id = ThreadId::parse(&thread.id)
            .map_err(|e| HarnessError::Other(format!("invalid thread id: {e}")))?;
        let checkpoint_dir = self.checkpoints_dir_for(&thread_id);
        for checkpoint_id in discarded_checkpoints {
            let _ = fs::remove_file(checkpoint_dir.join(format!("{checkpoint_id}.json")));
        }

        Ok(restored)
    }

    /// Restore the conversation to the point immediately before a user
    /// message so the caller can submit an edited replacement as a new turn.
    ///
    /// Each non-initial desktop turn creates a checkpoint at exactly this
    /// boundary. The returned thread intentionally excludes the selected
    /// message and every later response; the next send owns the replacement
    /// message id and creates a fresh checkpoint for the new branch.
    pub fn rewind_before_user_message(&self, thread: &Thread, message_id: &str) -> Result<Thread> {
        let message_index = thread
            .messages
            .iter()
            .position(|message| message.id() == message_id)
            .ok_or_else(|| HarnessError::Other("message not found".into()))?;
        if !matches!(
            thread.messages.get(message_index),
            Some(StoredMessage::User { .. })
        ) {
            return Err(HarnessError::Other(
                "only user messages can be edited".into(),
            ));
        }

        let mut restored = if message_index == 0 {
            let mut base = thread.clone();
            base.messages.clear();
            base.agent_messages.clear();
            base.title = None;
            base
        } else {
            let checkpoint = thread
                .checkpoints
                .iter()
                .rev()
                .find(|checkpoint| checkpoint.message_count == message_index)
                .ok_or_else(|| {
                    HarnessError::Other(
                        "message cannot be edited because its rewind checkpoint is missing".into(),
                    )
                })?;
            self.load_checkpoint(&thread.id, &checkpoint.id)?
        };

        if restored.messages.len() != message_index {
            return Err(HarnessError::Other(
                "message rewind checkpoint does not match the transcript".into(),
            ));
        }

        let thread_id = ThreadId::parse(&thread.id)
            .map_err(|e| HarnessError::Other(format!("invalid thread id: {e}")))?;
        let discarded_checkpoints = thread
            .checkpoints
            .iter()
            .filter(|checkpoint| checkpoint.message_count >= message_index)
            .map(|checkpoint| checkpoint.id.clone())
            .collect::<Vec<_>>();
        restored.checkpoints = thread
            .checkpoints
            .iter()
            .filter(|checkpoint| checkpoint.message_count < message_index)
            .cloned()
            .collect();
        restored.provider_session = None;
        restored.touch();
        self.save(&restored)?;

        let checkpoint_dir = self.checkpoints_dir_for(&thread_id);
        for checkpoint_id in discarded_checkpoints {
            let _ = fs::remove_file(checkpoint_dir.join(format!("{checkpoint_id}.json")));
        }

        Ok(restored)
    }

    /// Fork the current thread without sharing future checkpoint state.
    pub fn fork(&self, source: &Thread, title: Option<&str>) -> Result<Thread> {
        self.fork_with_provider(source, source.provider_id.as_deref(), title)
    }

    /// Fork the saved conversation state represented by one checkpoint. The
    /// original thread and its checkpoint files remain untouched.
    pub fn fork_from_checkpoint(
        &self,
        source: &Thread,
        checkpoint_id: &str,
        title: Option<&str>,
    ) -> Result<Thread> {
        if !source
            .checkpoints
            .iter()
            .any(|checkpoint| checkpoint.id == checkpoint_id)
        {
            return Err(HarnessError::Other(
                "checkpoint is not part of this conversation".into(),
            ));
        }
        let snapshot = self.load_checkpoint(&source.id, checkpoint_id)?;
        self.fork(&snapshot, title)
    }

    /// Fork a thread while assigning the copy to another provider.
    ///
    /// The source remains untouched. Provider adapters decide how the copied
    /// internal history is represented on the wire; the durable copy itself
    /// is explicitly owned by the target provider from its first turn.
    pub fn fork_for_provider(
        &self,
        source: &Thread,
        provider_id: &str,
        title: Option<&str>,
    ) -> Result<Thread> {
        self.fork_with_provider(source, Some(provider_id), title)
    }

    fn fork_with_provider(
        &self,
        source: &Thread,
        provider_id: Option<&str>,
        title: Option<&str>,
    ) -> Result<Thread> {
        let mut fork = source.clone();
        fork.id = new_id("thread");
        fork.created_at = now_secs();
        fork.updated_at = fork.created_at;
        fork.pinned = false;
        fork.provider_id = provider_id.map(str::to_string);
        // A fork owns a new provider conversation. Its canonical transcript is
        // copied, but a native continuation cursor must never be shared.
        fork.provider_session = None;
        fork.title = title
            .map(str::to_string)
            .or_else(|| source.title.as_ref().map(|t| format!("Copy of {t}")));
        fork.checkpoints.clear();
        self.save(&fork)?;
        Ok(fork)
    }

    /// Update a thread's sidebar pin without changing its activity timestamp.
    pub fn set_pinned(&self, id: &str, pinned: bool) -> Result<ThreadSummary> {
        let mut thread = self.load(id)?;
        if thread.set_pinned(pinned) {
            self.save(&thread)?;
        }
        Ok(thread.summary())
    }

    /// Rename a saved chat without changing its activity order.
    pub fn rename(&self, id: &str, title: &str) -> Result<ThreadSummary> {
        let mut thread = self.load(id)?;
        let changed = thread.set_title(title).map_err(HarnessError::Other)?;
        if changed {
            self.save(&thread)?;
        }
        Ok(thread.summary())
    }

    /// Permanently remove a thread file. Missing files are success (idempotent).
    pub fn delete(&self, id: &str) -> Result<()> {
        let tid = ThreadId::parse(id)
            .map_err(|e| HarnessError::Other(format!("invalid thread id: {e}")))?;
        let path = self.path_for(&tid);
        match fs::remove_file(&path) {
            Ok(()) => {
                let _ = fs::remove_dir_all(self.checkpoints_dir_for(&tid));
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(HarnessError::Other(format!(
                "delete thread {}: {e}",
                path.display()
            ))),
        }
    }

    pub fn list(&self) -> Result<Vec<ThreadSummary>> {
        self.list_filtered(None)
    }

    /// Recent threads for one provider (active-provider filter).
    pub fn list_for_provider(&self, provider_id: &str) -> Result<Vec<ThreadSummary>> {
        self.list_filtered(Some(provider_id))
    }

    fn list_filtered(&self, provider_id: Option<&str>) -> Result<Vec<ThreadSummary>> {
        let mut out = Vec::new();
        let entries = fs::read_dir(&self.dir).map_err(|e| {
            HarnessError::Other(format!("list threads {}: {e}", self.dir.display()))
        })?;
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            // Skip temps and preserved corrupt siblings.
            if !name.ends_with(".json") || name.contains(".corrupt") {
                continue;
            }
            let Ok(body) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(thread) = serde_json::from_str::<Thread>(&body) else {
                continue;
            };
            // Skip unsupported newer versions rather than rewriting them.
            if thread.version > THREAD_FORMAT_VERSION {
                continue;
            }
            if let Some(want) = provider_id {
                match thread.provider_id.as_deref() {
                    Some(id) if id == want => {}
                    _ => continue,
                }
            }
            out.push(thread.summary());
        }
        out.sort_by_key(|b| std::cmp::Reverse(b.updated_at));
        Ok(out)
    }
}

/// Rename a corrupt thread file aside so it is not overwritten.
fn preserve_corrupt(path: &Path) -> Result<PathBuf> {
    let stamp = now_secs();
    let preserved = path.with_extension(format!("json.corrupt-{stamp}"));
    fs::rename(path, &preserved).map_err(|e| {
        HarnessError::Other(format!("preserve corrupt thread {}: {e}", path.display()))
    })?;
    Ok(preserved)
}

#[cfg(test)]
mod characterization {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zest-thread-{name}-{}", new_id("tmp")));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn store_create_save_load_round_trip() {
        let root = scratch("roundtrip");
        let store = ThreadStore::open(&root).unwrap();
        assert!(store.dir().ends_with(Path::new(".zest").join("threads")));

        let mut thread = store.create().unwrap();
        thread.apply_user("user-1", "first question about the repo");
        thread.apply_text_delta("asst-1", "hello ");
        thread.apply_text_delta("asst-1", "world");
        thread.apply_done("asst-1");
        store.save(&thread).unwrap();

        let loaded = store.load(&thread.id).unwrap();
        assert_eq!(loaded.id, thread.id);
        assert_eq!(
            loaded.title.as_deref(),
            Some("first question about the repo")
        );
        assert_eq!(loaded.messages.len(), 2);
        match &loaded.messages[0] {
            StoredMessage::User { id, text } => {
                assert_eq!(id, "user-1");
                assert_eq!(text, "first question about the repo");
            }
            other => panic!("expected user message, got {other:?}"),
        }
        match &loaded.messages[1] {
            StoredMessage::Assistant {
                id,
                text,
                streaming,
                ..
            } => {
                assert_eq!(id, "asst-1");
                assert_eq!(text, "hello world");
                assert!(!streaming);
            }
            other => panic!("expected assistant message, got {other:?}"),
        }

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, thread.id);
        assert_eq!(listed[0].message_count, 2);
    }

    #[test]
    fn git_context_round_trips_and_stays_attached_to_summary() {
        let root = scratch("git-context");
        let store = ThreadStore::open(&root).unwrap();
        let mut thread = store.create_for_provider("codex").unwrap();

        assert!(thread.ensure_git_context(Some("master".into()), Some("abc123".into())));
        assert!(thread.record_git_branch(Some("feat/live-indicator".into())));
        assert!(!thread.record_git_branch(Some("feat/live-indicator".into())));
        thread.record_git_context(ThreadGitContext {
            base_branch: Some("master".into()),
            branch: Some("feat/live-indicator".into()),
            start_commit: Some("abc123".into()),
            pull_request: Some(PullRequestLink {
                repository: None,
                number: 5,
                title: "Live branch indicator".into(),
                url: "https://github.com/example/repo/pull/5".into(),
                state: "OPEN".into(),
                is_draft: false,
                additions: 65,
                deletions: 1,
                changed_files: 3,
            }),
        });
        store.save(&thread).unwrap();

        let loaded = store.load(&thread.id).unwrap();
        assert_eq!(loaded.git_context, thread.git_context);
        assert_eq!(loaded.summary().git_context, thread.git_context);
    }

    /// Twenty-four checkpoints of a large conversation is twenty-four complete
    /// copies of it. A count cap alone never noticed that.
    #[test]
    fn checkpoints_are_bounded_by_bytes_not_only_by_count() {
        let root = scratch("checkpoint-bytes");
        let store = ThreadStore::open(&root).unwrap();
        let mut thread = store.create_for_provider("codex").unwrap();

        // One fat message, so a handful of snapshots clears the byte ceiling
        // long before the count ceiling is anywhere near.
        thread.apply_user("u1", &"x".repeat(12 * 1024 * 1024));
        store.save(&thread).unwrap();

        for index in 0..8 {
            store
                .create_checkpoint(&mut thread, format!("turn {index}"))
                .unwrap();
        }

        let dir = store.checkpoints_dir_for(&ThreadId::parse(&thread.id).unwrap());
        let on_disk: u64 = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter_map(|entry| entry.metadata().ok())
            .map(|meta| meta.len())
            .sum();

        assert!(
            thread.checkpoints.len() < 8,
            "the byte ceiling pruned before the count ceiling could: {}",
            thread.checkpoints.len()
        );
        assert!(
            on_disk <= ThreadStore::MAX_CHECKPOINT_BYTES,
            "kept {on_disk} bytes"
        );
        // Metadata and files must agree, or rewind offers a checkpoint that is
        // no longer there.
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            thread.checkpoints.len()
        );
    }

    /// Even a single oversized snapshot survives: deleting it would buy space
    /// by making rewind impossible, which is the one thing it exists for.
    #[test]
    fn the_last_checkpoint_is_never_pruned() {
        let root = scratch("checkpoint-last");
        let store = ThreadStore::open(&root).unwrap();
        let mut thread = store.create_for_provider("codex").unwrap();
        let huge = ThreadStore::MAX_CHECKPOINT_BYTES as usize;
        thread.apply_user("u1", &"y".repeat(huge));
        store.save(&thread).unwrap();

        store.create_checkpoint(&mut thread, "only one").unwrap();
        assert_eq!(thread.checkpoints.len(), 1);
    }

    #[test]
    fn checkpoints_restore_wire_history_and_forks_start_clean() {
        let root = scratch("checkpoint");
        let store = ThreadStore::open(&root).unwrap();
        let mut thread = store.create_for_provider("codex").unwrap();
        thread.apply_user("u1", "first question");
        thread.apply_assistant_start("a1", None);
        thread.apply_text_delta("a1", "first answer");
        thread.apply_done("a1");
        thread.agent_messages = vec![Message::user_text("first question")];
        store.save(&thread).unwrap();

        let checkpoint = store
            .create_checkpoint(&mut thread, "Before the next turn")
            .unwrap();
        assert_eq!(thread.checkpoints.len(), 1);
        assert_eq!(checkpoint.message_count, 2);
        assert!(
            store
                .checkpoints_dir_for(&ThreadId::parse(&thread.id).unwrap())
                .join(format!("{}.json", checkpoint.id))
                .is_file(),
            "the snapshot file backs the metadata record"
        );

        thread.apply_user("u2", "second question");
        store.save(&thread).unwrap();
        let restored = store.load_checkpoint(&thread.id, &checkpoint.id).unwrap();
        assert_eq!(restored.id, thread.id);
        assert_eq!(restored.messages.len(), 2);
        assert_eq!(restored.agent_messages.len(), 1);
        assert!(restored.checkpoints.is_empty());

        let fork = store.fork(&thread, None).unwrap();
        assert_ne!(fork.id, thread.id);
        assert_eq!(fork.provider_id.as_deref(), Some("codex"));
        assert!(fork.title.as_deref().unwrap().starts_with("Copy of "));
        assert!(fork.checkpoints.is_empty());
        assert!(store.load(&fork.id).is_ok());

        let deepseek_copy = store
            .fork_for_provider(&thread, "deepseek", Some("Copy for DeepSeek"))
            .unwrap();
        assert_eq!(deepseek_copy.provider_id.as_deref(), Some("deepseek"));
        assert_eq!(deepseek_copy.title.as_deref(), Some("Copy for DeepSeek"));
        assert_eq!(
            serde_json::to_string(&deepseek_copy.agent_messages).unwrap(),
            serde_json::to_string(&thread.agent_messages).unwrap()
        );
        assert!(store.load(&deepseek_copy.id).is_ok());

        store.delete(&thread.id).unwrap();
        assert!(!store
            .checkpoints_dir_for(&ThreadId::parse(&thread.id).unwrap())
            .exists());
    }

    #[test]
    fn editing_a_user_message_restores_the_checkpoint_before_its_turn() {
        let root = scratch("edit-message");
        let store = ThreadStore::open(&root).unwrap();
        let mut thread = store.create_for_provider("codex").unwrap();

        thread.apply_user("u1", "first question");
        thread.apply_assistant_start("a1", None);
        thread.apply_text_delta("a1", "first answer");
        thread.apply_done("a1");
        thread.agent_messages = vec![Message::user_text("first question")];
        store.save(&thread).unwrap();

        let checkpoint = store
            .create_checkpoint(&mut thread, "Before the next turn")
            .unwrap();
        thread.apply_user("u2", "second question");
        thread.apply_assistant_start("a2", None);
        thread.apply_text_delta("a2", "second answer");
        thread.apply_done("a2");
        thread
            .agent_messages
            .push(Message::user_text("second question"));
        store.save(&thread).unwrap();

        let restored = store.rewind_before_user_message(&thread, "u2").unwrap();
        assert_eq!(restored.messages.len(), 2);
        assert_eq!(restored.messages[0].id(), "u1");
        assert_eq!(restored.messages[1].id(), "a1");
        assert_eq!(restored.agent_messages.len(), 1);
        assert!(restored.checkpoints.is_empty());
        assert!(store.load_checkpoint(&thread.id, &checkpoint.id).is_err());

        let loaded = store.load(&thread.id).unwrap();
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.agent_messages.len(), 1);
    }

    #[test]
    fn rewinding_prunes_later_snapshots_and_clears_native_session_state() {
        let root = scratch("rewind-checkpoint");
        let store = ThreadStore::open(&root).unwrap();
        let mut thread = store.create_for_provider("codex").unwrap();
        thread.apply_user("u1", "first question");
        thread.apply_assistant_start("a1", None);
        thread.apply_text_delta("a1", "first answer");
        thread.apply_done("a1");
        store.save(&thread).unwrap();

        let first = store.create_checkpoint(&mut thread, "first").unwrap();
        thread.apply_user("u2", "second question");
        store.save(&thread).unwrap();
        let second = store.create_checkpoint(&mut thread, "second").unwrap();
        thread.provider_session = Some(crate::provider::ProviderSessionRef::CodexAppServer {
            thread_id: "native-thread".into(),
        });
        store.save(&thread).unwrap();

        let restored = store.rewind_to_checkpoint(&thread, &first.id).unwrap();

        assert!(restored.provider_session.is_none());
        assert_eq!(restored.checkpoints.len(), 1);
        assert_eq!(restored.checkpoints[0].id, first.id);
        assert!(store.load_checkpoint(&thread.id, &second.id).is_err());
        assert!(store.load_checkpoint(&thread.id, &first.id).is_ok());
    }

    #[test]
    fn editing_the_first_user_message_starts_from_an_empty_branch() {
        let root = scratch("edit-first-message");
        let store = ThreadStore::open(&root).unwrap();
        let mut thread = store.create_for_provider("codex").unwrap();
        thread.apply_user("u1", "first question");
        thread.apply_assistant_start("a1", None);
        thread.apply_text_delta("a1", "first answer");
        thread.apply_done("a1");
        thread.agent_messages = vec![Message::user_text("first question")];
        store.save(&thread).unwrap();

        let restored = store.rewind_before_user_message(&thread, "u1").unwrap();
        assert!(restored.messages.is_empty());
        assert!(restored.agent_messages.is_empty());
        assert!(restored.title.is_none());
        assert_eq!(restored.provider_id.as_deref(), Some("codex"));
    }

    #[test]
    fn apply_chat_event_upserts_preserve_tool_and_approval_fields() {
        let mut thread = Thread::new();
        thread.apply_user("u1", "edit the file");
        thread.apply_thinking_delta("a1", "planning…");
        thread.apply_text_delta("a1", "I'll write");
        thread.apply_tool_start("a1", "tool-1", "write_file");
        thread.apply_approval_needed(
            "a1",
            "tool-1",
            "write_file",
            "approval-1",
            "src/main.rs",
            "write src/main.rs",
            "@@ -1 +1 @@\n-old\n+new\n",
        );

        match &thread.messages[1] {
            StoredMessage::Assistant {
                thinking,
                text,
                tools,
                streaming,
                ..
            } => {
                assert_eq!(thinking, "planning…");
                assert_eq!(text, "I'll write");
                assert!(*streaming);
                assert_eq!(tools.len(), 1);
                assert_eq!(tools[0].id, "tool-1");
                assert_eq!(tools[0].status, "awaiting_approval");
                assert_eq!(tools[0].approval_id.as_deref(), Some("approval-1"));
                assert_eq!(tools[0].path.as_deref(), Some("src/main.rs"));
                assert_eq!(tools[0].summary.as_deref(), Some("write src/main.rs"));
                assert!(tools[0].diff.as_ref().unwrap().contains("+new"));
            }
            other => panic!("expected assistant, got {other:?}"),
        }

        // Duplicate tool_start is a no-op for the same id.
        thread.apply_tool_start("a1", "tool-1", "write_file");
        assert_eq!(
            match &thread.messages[1] {
                StoredMessage::Assistant { tools, .. } => tools.len(),
                _ => 0,
            },
            1
        );

        thread.apply_tool_result(
            "a1",
            "tool-1",
            "write_file",
            "wrote src/main.rs",
            false,
            None,
            None,
            None,
        );
        match &thread.messages[1] {
            StoredMessage::Assistant { tools, .. } => {
                assert_eq!(tools[0].status, "done");
                assert_eq!(tools[0].summary.as_deref(), Some("wrote src/main.rs"));
                assert!(tools[0].approval_id.is_none());
                // Path/diff retained after allow for card context.
                assert_eq!(tools[0].path.as_deref(), Some("src/main.rs"));
                assert!(tools[0].diff.is_some());
            }
            other => panic!("expected assistant, got {other:?}"),
        }

        thread.apply_error("a1", "upstream failed");
        match &thread.messages[1] {
            StoredMessage::Assistant {
                error, streaming, ..
            } => {
                assert_eq!(error.as_deref(), Some("upstream failed"));
                assert!(!streaming);
            }
            other => panic!("expected assistant, got {other:?}"),
        }
    }

    #[test]
    fn load_or_none_and_duplicate_user_id_are_stable() {
        let root = scratch("stable");
        let store = ThreadStore::open(&root).unwrap();
        assert!(store.load_or_none("missing").is_none());

        let mut thread = Thread::new();
        thread.apply_user("u1", "hello");
        thread.apply_user("u1", "ignored duplicate");
        assert_eq!(thread.messages.len(), 1);
        match &thread.messages[0] {
            StoredMessage::User { text, .. } => assert_eq!(text, "hello"),
            other => panic!("expected user, got {other:?}"),
        }
        // First user text still owns the title.
        assert_eq!(thread.title.as_deref(), Some("hello"));
    }

    #[test]
    fn stored_message_json_uses_role_tag_and_camel_case_tools() {
        let mut thread = Thread::new();
        thread.apply_user("u1", "hi");
        thread.apply_approval_needed(
            "a1",
            "t1",
            "write_file",
            "approval-1",
            "f.txt",
            "write f.txt",
            "diff",
        );
        let json = serde_json::to_value(&thread).unwrap();
        assert_eq!(json["messages"][0]["role"], "user");
        assert_eq!(json["messages"][1]["role"], "assistant");
        // ToolPart optional fields are camelCase on the wire.
        let tool = &json["messages"][1]["tools"][0];
        assert_eq!(tool["status"], "awaiting_approval");
        assert_eq!(tool["approvalId"], "approval-1");
        assert_eq!(tool["path"], "f.txt");
        assert!(tool.get("approval_id").is_none());
    }

    #[test]
    fn thread_id_rejects_traversal_and_drive_prefixes() {
        assert!(ThreadId::parse("thread-abc-1").is_ok());
        assert!(ThreadId::parse("../secret").is_err());
        assert!(ThreadId::parse("..\\secret").is_err());
        assert!(ThreadId::parse("C:windows").is_err());
        assert!(ThreadId::parse("foo/bar").is_err());
        assert!(ThreadId::parse(".").is_err());
        assert!(ThreadId::parse("..").is_err());
        assert!(ThreadId::parse("").is_err());
        assert!(ThreadId::parse("has space").is_err());
    }

    #[test]
    fn store_rejects_traversal_thread_id() {
        let root = scratch("traverse");
        let store = ThreadStore::open(&root).unwrap();
        let err = store.load("../outside").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid thread id"), "{msg}");
    }

    #[test]
    fn store_delete_removes_file_and_is_idempotent() {
        let root = scratch("delete");
        let store = ThreadStore::open(&root).unwrap();
        let thread = store.create_for_provider("codex").unwrap();
        let path = store.dir().join(format!("{}.json", thread.id));
        assert!(path.exists());
        store.delete(&thread.id).unwrap();
        assert!(!path.exists());
        store.delete(&thread.id).unwrap(); // idempotent
        assert!(store.delete("../outside").is_err());
    }

    #[test]
    fn store_pin_round_trips_without_changing_activity_order() {
        let root = scratch("pin");
        let store = ThreadStore::open(&root).unwrap();
        let thread = store.create_for_provider("codex").unwrap();
        let before = thread.updated_at;

        let summary = store.set_pinned(&thread.id, true).unwrap();
        assert!(summary.pinned);
        assert_eq!(summary.updated_at, before);

        let loaded = store.load(&thread.id).unwrap();
        assert!(loaded.pinned);
        assert_eq!(loaded.updated_at, before);

        let unpinned = store.set_pinned(&thread.id, false).unwrap();
        assert!(!unpinned.pinned);
    }

    #[test]
    fn store_rename_round_trips_without_changing_activity_order() {
        let root = scratch("rename");
        let store = ThreadStore::open(&root).unwrap();
        let thread = store.create_for_provider("codex").unwrap();
        let before = thread.updated_at;

        let summary = store.rename(&thread.id, "  Release checklist  ").unwrap();
        assert_eq!(summary.title.as_deref(), Some("Release checklist"));
        assert_eq!(summary.updated_at, before);

        let loaded = store.load(&thread.id).unwrap();
        assert_eq!(loaded.title.as_deref(), Some("Release checklist"));
        assert_eq!(loaded.updated_at, before);
        assert!(store.rename(&thread.id, "").is_err());
        assert!(store
            .rename(&thread.id, &"x".repeat(MAX_THREAD_TITLE_CHARS + 1))
            .is_err());
    }

    #[test]
    fn migrates_legacy_thread_json_without_version() {
        let root = scratch("migrate");
        let store = ThreadStore::open(&root).unwrap();
        let id = "thread-legacy1";
        let path = store.dir().join(format!("{id}.json"));
        fs::write(
            &path,
            r#"{
  "id": "thread-legacy1",
  "createdAt": 1,
  "updatedAt": 2,
  "messages": [{"role":"user","id":"u1","text":"hi"}],
  "agentMessages": []
}"#,
        )
        .unwrap();

        let loaded = store.load_with_recovery(id).unwrap();
        assert_eq!(loaded.thread.version, THREAD_FORMAT_VERSION);
        assert_eq!(loaded.thread.wire_format, WIRE_FORMAT_ANTHROPIC_MESSAGES);
        assert!(loaded.warning.is_some());
    }

    #[test]
    fn refuses_newer_thread_format() {
        let root = scratch("newer");
        let store = ThreadStore::open(&root).unwrap();
        let id = "thread-newer1";
        let path = store.dir().join(format!("{id}.json"));
        fs::write(
            &path,
            r#"{
  "version": 99,
  "id": "thread-newer1",
  "createdAt": 1,
  "updatedAt": 2,
  "providerId": "codex",
  "wireFormat": "anthropic_messages",
  "messages": [],
  "agentMessages": []
}"#,
        )
        .unwrap();
        let err = store.load_typed(id).unwrap_err();
        assert!(
            matches!(err, ThreadLoadError::UnsupportedVersion { .. }),
            "{err}"
        );
        // Original file must remain (no rewrite).
        assert!(path.exists());
    }

    #[test]
    fn corrupt_thread_is_preserved_aside() {
        let root = scratch("corrupt");
        let store = ThreadStore::open(&root).unwrap();
        let id = "thread-bad1";
        let path = store.dir().join(format!("{id}.json"));
        fs::write(&path, "{not json").unwrap();
        let err = store.load_with_recovery(id).unwrap_err().to_string();
        assert!(err.contains("corrupt"), "{err}");
        assert!(!path.exists());
        let preserved: Vec<_> = fs::read_dir(store.dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("corrupt"))
            .collect();
        assert_eq!(preserved.len(), 1);
    }

    #[test]
    fn terminalize_interrupted_closes_running_and_approvals() {
        let mut thread = Thread::new();
        thread.apply_tool_start("a1", "t1", "write_file");
        thread.apply_approval_needed("a1", "t2", "write_file", "ap1", "f.txt", "write", "diff");
        assert!(thread.terminalize_interrupted());
        match &thread.messages[0] {
            StoredMessage::Assistant {
                tools, streaming, ..
            } => {
                assert!(!streaming);
                assert_eq!(tools[0].status, "error");
                assert!(tools[0].summary.as_deref().unwrap().contains("interrupted"));
                assert_eq!(tools[1].status, "error");
                assert!(tools[1]
                    .summary
                    .as_deref()
                    .unwrap()
                    .contains("approval interrupted"));
                assert!(tools[1].approval_id.is_none());
            }
            other => panic!("expected assistant, got {other:?}"),
        }
    }
    /// A plan reopened tomorrow must still look like a plan; otherwise the
    /// card silently degrades to plain text and reads as a rendering bug.
    #[test]
    fn the_command_that_produced_a_turn_survives_a_reload() {
        let mut thread = Thread::new();
        thread.apply_assistant_start("a1", Some("plan"));
        thread.apply_text_delta("a1", "# Plan");

        let json = serde_json::to_string(&thread).unwrap();
        let back: Thread = serde_json::from_str(&json).unwrap();
        match &back.messages[0] {
            StoredMessage::Assistant { command, text, .. } => {
                assert_eq!(command.as_deref(), Some("plan"));
                assert_eq!(text, "# Plan");
            }
            other => panic!("expected assistant, got {other:?}"),
        }
    }

    #[test]
    fn an_ordinary_turn_stores_no_command_and_older_threads_still_load() {
        let mut thread = Thread::new();
        thread.apply_assistant_start("a1", None);
        let json = serde_json::to_string(&thread).unwrap();
        // Omitted rather than null, so the field adds nothing to every message.
        assert!(!json.contains("command"), "{json}");

        // A thread written before the field existed must still deserialize —
        // the field is new, and old threads on disk have never heard of it.
        let legacy = r#"{"version":1,"id":"t1","createdAt":1,"updatedAt":1,
"providerId":"codex","wireFormat":"anthropic_messages","agentMessages":[],
"messages":[{"role":"assistant","id":"a1","text":"hi","thinking":"",
"tools":[],"streaming":false}]}"#;
        let back: Thread = serde_json::from_str(legacy).expect("older threads still load");
        match &back.messages[0] {
            StoredMessage::Assistant { command, text, .. } => {
                assert_eq!(*command, None);
                assert_eq!(text, "hi");
            }
            other => panic!("expected assistant, got {other:?}"),
        }
    }
}
