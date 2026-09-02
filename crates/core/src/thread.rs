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

use serde::de::{Deserializer, IgnoredAny, SeqAccess, Visitor};
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
/// v4 adds durable pending inputs and the thin lifecycle ledger.
pub const THREAD_FORMAT_VERSION: u32 = 4;

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
        /// Provider to repair or replace when the error was an auth failure
        /// that Zest cannot fix with its own managed login flow.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_selection: Option<String>,
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

    /// Visible transcript text used by chat search.
    pub fn searchable_text(&self) -> &str {
        match self {
            Self::User { text, .. } | Self::Assistant { text, .. } => text,
        }
    }
}

/// First matching window around `query`, collapsed to one line for palette rows.
pub fn match_excerpt(text: &str, query: &str) -> Option<String> {
    const RADIUS: usize = 42;
    let needle = query.trim();
    if needle.is_empty() {
        return None;
    }
    let haystack = text.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let byte_at = haystack.find(&needle)?;
    let match_end = byte_at + needle.len();
    if !text.is_char_boundary(byte_at) || !text.is_char_boundary(match_end) {
        return None;
    }
    let char_start = text[..byte_at].chars().count();
    let match_chars = text[byte_at..match_end].chars().count();
    let chars: Vec<char> = text.chars().collect();
    let start = char_start.saturating_sub(RADIUS);
    let end = (char_start + match_chars + RADIUS).min(chars.len());
    let mut snippet: String = chars[start..end].iter().collect();
    snippet = snippet.split_whitespace().collect::<Vec<_>>().join(" ");
    if snippet.is_empty() {
        return None;
    }
    if start > 0 {
        snippet.insert(0, '…');
    }
    if end < chars.len() {
        snippet.push('…');
    }
    Some(snippet)
}

pub fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let needle = needle.trim();
    !needle.is_empty()
        && haystack
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
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
    /// Number of pending followup/steer/inject inputs owned by this thread.
    #[serde(default)]
    pub pending_input_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_context: Option<ThreadGitContext>,
}

/// JSON array whose elements are discarded; only the length is kept.
#[derive(Default)]
struct CountedSeq(usize);

impl<'de> Deserialize<'de> for CountedSeq {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct CountingVisitor;
        impl<'de> Visitor<'de> for CountingVisitor {
            type Value = CountedSeq;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a JSON array")
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<CountedSeq, A::Error> {
                let mut n = 0usize;
                while seq.next_element::<IgnoredAny>()?.is_some() {
                    n += 1;
                }
                Ok(CountedSeq(n))
            }
        }
        deserializer.deserialize_seq(CountingVisitor)
    }
}

/// Named field skipped without allocating. Implements Default so missing
/// `skip_serializing_if` fields still deserialize.
#[derive(Default)]
struct IgnoredField;

impl<'de> Deserialize<'de> for IgnoredField {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        IgnoredAny::deserialize(deserializer)?;
        Ok(IgnoredField)
    }
}

/// Sidebar listing projection: count messages, skip wire history and ledger.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadListing {
    #[serde(default)]
    version: u32,
    id: String,
    created_at: u64,
    updated_at: u64,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    provider_id: Option<String>,
    #[serde(default)]
    git_context: Option<ThreadGitContext>,
    #[serde(default)]
    messages: CountedSeq,
    #[serde(default)]
    pending_inputs: CountedSeq,
    #[serde(default)]
    #[allow(dead_code)]
    agent_messages: IgnoredField,
    #[serde(default)]
    #[allow(dead_code)]
    events: IgnoredField,
    #[serde(default)]
    #[allow(dead_code)]
    checkpoints: IgnoredField,
    #[serde(default)]
    #[allow(dead_code)]
    provider_session: IgnoredField,
}

impl ThreadListing {
    fn into_summary(self) -> Option<ThreadSummary> {
        if self.version > THREAD_FORMAT_VERSION {
            return None;
        }
        Some(ThreadSummary {
            id: self.id,
            created_at: self.created_at,
            updated_at: self.updated_at,
            title: self.title,
            pinned: self.pinned,
            provider_id: self.provider_id,
            message_count: self.messages.0,
            pending_input_count: self.pending_inputs.0,
            git_context: self.git_context,
        })
    }
}

/// Parse sidebar metadata without allocating transcript or wire-history Vecs.
pub fn thread_summary_from_json(body: &str) -> Option<ThreadSummary> {
    serde_json::from_str::<ThreadListing>(body)
        .ok()
        .and_then(ThreadListing::into_summary)
}

/// Where a user or the runtime wants an input delivered.
///
/// This is deliberately a small enum instead of three ad-hoc queues. The
/// distinction is the delivery contract: followups wait for the current turn
/// to finish, while steer/inject inputs are claimed at the next provider step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadInputTarget {
    Followup,
    Steer,
    Inject,
}

/// An input that has been accepted by the thread but has not yet been
/// delivered to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadInput {
    pub id: String,
    pub target: ThreadInputTarget,
    pub text: String,
    pub created_at: u64,
    /// Prepared attachments are kept in the thread so an attachment-only
    /// followup survives a restart too. The desktop converts this core shape
    /// back to its provider-facing input just before execution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ThreadInputAttachment>,
}

/// Provider-neutral attachment payload for a durable queued input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadInputAttachment {
    pub name: String,
    pub detail: String,
    pub content: Option<String>,
    pub status: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub data_base64: Option<String>,
}

/// Thin, append-only lifecycle metadata. Transcript text and tool bodies stay
/// in the existing snapshot; this ledger only records enough identity to
/// repair and replay a turn after a crash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThreadEventKind {
    TurnStarted {
        turn_id: String,
    },
    ToolCalled {
        turn_id: String,
        call_id: String,
        name: String,
    },
    ApprovalRequested {
        turn_id: String,
        approval_id: String,
        call_id: String,
        name: String,
    },
    ToolResult {
        turn_id: String,
        call_id: String,
        name: String,
        is_error: bool,
    },
    TurnCompleted {
        turn_id: String,
        status: String,
    },
    InputQueued {
        input_id: String,
        target: ThreadInputTarget,
    },
    InputClaimed {
        input_id: String,
        target: ThreadInputTarget,
    },
    InputCancelled {
        input_id: String,
        target: ThreadInputTarget,
    },
    JobStarted {
        job_id: String,
        kind: String,
    },
    JobCompleted {
        job_id: String,
        status: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadEvent {
    pub id: String,
    pub sequence: u64,
    pub at: u64,
    pub event: ThreadEventKind,
}

/// The reason a checkpoint exists. The UI uses this to give turn checkpoints
/// and maintenance checkpoints slightly different affordances without
/// inspecting labels written by a caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThreadCheckpointKind {
    #[default]
    Turn,
    Compaction,
    Manual,
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
    #[error(
        "This chat was started with a different Codex sign-in. Start a new chat to keep using the current one."
    )]
    ProviderKindMismatch {
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
    /// Config kind that started this conversation. Missing on older files.
    /// A Codex chat without a kind is treated as `codex_cli`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_kind: Option<String>,
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
    /// Inputs accepted while a turn was busy, or while the runtime was
    /// restarting. The queue is part of the thread snapshot, never webview
    /// state.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_inputs: Vec<ThreadInput>,
    /// Compact lifecycle metadata used for crash repair and transcript replay.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<ThreadEvent>,
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
            provider_kind: None,
            provider_session: None,
            git_context: None,
            wire_format: default_wire_format(),
            messages: Vec::new(),
            checkpoints: Vec::new(),
            agent_messages: Vec::new(),
            pending_inputs: Vec::new(),
            events: Vec::new(),
        }
    }

    pub fn with_provider(mut self, provider_id: impl Into<String>) -> Self {
        self.provider_id = Some(provider_id.into());
        self
    }

    pub fn with_provider_kind(mut self, provider_kind: impl Into<String>) -> Self {
        self.provider_kind = Some(provider_kind.into());
        self
    }

    /// Stored kind, or `codex_cli` when a Codex chat predates this field.
    pub fn effective_provider_kind(&self) -> Option<&str> {
        if let Some(kind) = self.provider_kind.as_deref() {
            return Some(kind);
        }
        if self.provider_id.as_deref() == Some("codex") {
            return Some("codex_cli");
        }
        None
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

    /// Move this chat onto another Zest-loop provider. Drops the native
    /// continuation cursor so it cannot be sent to the new backend.
    pub fn reassign_provider(&mut self, provider_id: &str, provider_kind: Option<&str>) {
        self.provider_id = Some(provider_id.to_string());
        self.provider_kind = provider_kind.map(str::to_string);
        self.provider_session = None;
    }

    /// Copy this transcript into a new thread owned by another provider.
    ///
    /// The source is left untouched. The copy drops the native continuation
    /// cursor, pending inputs, and checkpoints so they cannot run twice.
    pub fn fork_for_provider(&self, provider_id: &str, provider_kind: Option<&str>) -> Thread {
        let now = now_secs();
        let mut fork = self.clone();
        fork.id = new_id("thread");
        fork.created_at = now;
        fork.updated_at = now;
        fork.pinned = false;
        fork.pending_inputs.clear();
        fork.checkpoints.clear();
        fork.title = self.title.as_ref().map(|title| format!("Copy of {title}"));
        let kind = match provider_kind {
            Some(kind) => Some(kind),
            None if self.provider_id.as_deref() == Some(provider_id) => {
                self.provider_kind.as_deref()
            }
            None => None,
        };
        fork.reassign_provider(provider_id, kind);
        fork
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

    pub fn assert_provider_kind(
        &self,
        provider_kind: &str,
    ) -> std::result::Result<(), ThreadLoadError> {
        match self.effective_provider_kind() {
            None => Ok(()),
            Some(owned) if owned == provider_kind => Ok(()),
            Some(owned) => Err(ThreadLoadError::ProviderKindMismatch {
                id: self.id.clone(),
                owned: owned.to_string(),
                wanted: provider_kind.to_string(),
            }),
        }
    }

    pub fn ensure_provider_kind(
        &mut self,
        provider_kind: &str,
    ) -> std::result::Result<(), ThreadLoadError> {
        self.assert_provider_kind(provider_kind)?;
        if self.provider_kind.is_none() {
            self.provider_kind = Some(provider_kind.to_string());
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
            pending_input_count: self.pending_inputs.len(),
            git_context: self.git_context.clone(),
        }
    }

    /// Snippet from the first user or assistant message that contains `query`.
    pub fn search_excerpt(&self, query: &str) -> Option<String> {
        self.search_match(query).map(|(_, snippet)| snippet)
    }

    /// First matching UI message and the snippet shown in the palette.
    pub fn search_match(&self, query: &str) -> Option<(&str, String)> {
        self.messages.iter().find_map(|msg| {
            match_excerpt(msg.searchable_text(), query).map(|snippet| (msg.id(), snippet))
        })
    }

    /// Append one lifecycle record and return its durable identity.
    ///
    /// The sequence is derived from the last record rather than from a
    /// process-global counter, so loading a thread in another process cannot
    /// create duplicate sequence numbers.
    pub fn record_event(&mut self, event: ThreadEventKind) -> String {
        let id = new_id("event");
        let sequence = self
            .events
            .last()
            .map(|entry| entry.sequence.saturating_add(1))
            .unwrap_or(1);
        self.events.push(ThreadEvent {
            id: id.clone(),
            sequence,
            at: now_secs(),
            event,
        });
        self.touch();
        id
    }

    /// Enqueue a validated input in FIFO order.
    pub fn enqueue_input(
        &mut self,
        target: ThreadInputTarget,
        text: impl Into<String>,
        attachments: Vec<ThreadInputAttachment>,
    ) -> std::result::Result<ThreadInput, String> {
        let text = text.into();
        if text.trim().is_empty() && attachments.is_empty() {
            return Err("queued input must contain text or an attachment".into());
        }
        let input = ThreadInput {
            id: new_id("input"),
            target,
            text,
            created_at: now_secs(),
            attachments,
        };
        self.pending_inputs.push(input.clone());
        self.record_event(ThreadEventKind::InputQueued {
            input_id: input.id.clone(),
            target,
        });
        Ok(input)
    }

    /// Update queued text without moving the input.
    pub fn update_input(
        &mut self,
        input_id: &str,
        text: impl Into<String>,
    ) -> std::result::Result<bool, String> {
        let text = text.into();
        let Some(input) = self
            .pending_inputs
            .iter_mut()
            .find(|input| input.id == input_id)
        else {
            return Ok(false);
        };
        if text.trim().is_empty() && input.attachments.is_empty() {
            return Err("queued input must contain text or an attachment".into());
        }
        if input.text == text {
            return Ok(false);
        }
        input.text = text;
        self.touch();
        Ok(true)
    }

    /// Remove one pending input and record why it left the durable queue.
    pub fn remove_input(&mut self, input_id: &str) -> bool {
        let Some(index) = self
            .pending_inputs
            .iter()
            .position(|input| input.id == input_id)
        else {
            return false;
        };
        let input = self.pending_inputs.remove(index);
        self.record_event(ThreadEventKind::InputCancelled {
            input_id: input.id,
            target: input.target,
        });
        true
    }

    /// Remove one input because the live agent is about to deliver it.
    pub fn claim_input(&mut self, input_id: &str) -> Option<ThreadInput> {
        let index = self
            .pending_inputs
            .iter()
            .position(|input| input.id == input_id)?;
        let input = self.pending_inputs.remove(index);
        self.record_event(ThreadEventKind::InputClaimed {
            input_id: input.id.clone(),
            target: input.target,
        });
        Some(input)
    }

    /// Claim all step-scoped inputs in insertion order. Followups remain in
    /// the queue until the active turn has completed.
    pub fn claim_next_step_inputs(&mut self) -> Vec<ThreadInput> {
        let mut claimed = Vec::new();
        let mut remaining = Vec::with_capacity(self.pending_inputs.len());
        for input in self.pending_inputs.drain(..) {
            if matches!(
                input.target,
                ThreadInputTarget::Steer | ThreadInputTarget::Inject
            ) {
                claimed.push(input);
            } else {
                remaining.push(input);
            }
        }
        self.pending_inputs = remaining;
        for input in &claimed {
            self.record_event(ThreadEventKind::InputClaimed {
                input_id: input.id.clone(),
                target: input.target,
            });
        }
        if !claimed.is_empty() {
            self.touch();
        }
        claimed
    }

    /// Claim the oldest followup for the next turn.
    pub fn claim_followup(&mut self) -> Option<ThreadInput> {
        let index = self
            .pending_inputs
            .iter()
            .position(|input| input.target == ThreadInputTarget::Followup)?;
        let input = self.pending_inputs.remove(index);
        self.record_event(ThreadEventKind::InputClaimed {
            input_id: input.id.clone(),
            target: input.target,
        });
        Some(input)
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
            provider_selection: None,
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
        self.apply_error_with_provider(message_id, message, None);
    }

    pub fn apply_error_with_provider(
        &mut self,
        message_id: &str,
        message: &str,
        provider_selection: Option<&str>,
    ) {
        self.ensure_assistant(message_id);
        if let Some(StoredMessage::Assistant {
            error,
            provider_selection: stored_provider_selection,
            streaming,
            ..
        }) = self.find_mut(message_id)
        {
            *error = Some(message.to_string());
            *stored_provider_selection = provider_selection.map(str::to_string);
            *streaming = false;
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

    /// Whether this id already has a row on disk.
    ///
    /// A different question from `load(..).is_ok()`, and deliberately so: this
    /// asks whether writing would *update* a chat or *create* one, which is
    /// what a caller holding a possibly-unsaved draft needs to know. It also
    /// costs a stat rather than a read-and-parse, so it is safe on a poll.
    pub fn exists(&self, id: &str) -> bool {
        ThreadId::parse(id).is_ok_and(|tid| self.path_for(&tid).exists())
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
        self.create_for_provider_kind(provider_id, None)
    }

    pub fn create_for_provider_kind(
        &self,
        provider_id: &str,
        provider_kind: Option<&str>,
    ) -> Result<Thread> {
        let mut thread = Thread::new().with_provider(provider_id);
        if let Some(kind) = provider_kind {
            thread.provider_kind = Some(kind.to_string());
        }
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

    /// Drop snapshots until both ceilings are satisfied, automatic ones first.
    ///
    /// Always leaves one behind. A thread with a single enormous checkpoint has
    /// nothing useful to delete — removing it would buy space by making rewind
    /// impossible, which is the one thing checkpoints exist for.
    ///
    /// Within that, a `Manual` checkpoint goes last. `Turn` and `Compaction`
    /// snapshots are bookkeeping the harness writes on its own and will write
    /// again; a manual one is a restore point somebody deliberately marked, and
    /// on a long thread the automatic ones would otherwise push it out of the
    /// window within a day's work. The ceilings still bind — when every
    /// remaining snapshot is manual, the oldest of those goes.
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
            // Checkpoints are ordered oldest-first, so the first non-manual
            // entry is the oldest automatic one. Falling back to index 0 keeps
            // the ceilings enforceable on an all-manual thread.
            let victim = thread
                .checkpoints
                .iter()
                .position(|checkpoint| checkpoint.kind != ThreadCheckpointKind::Manual)
                .unwrap_or(0);
            let dropped = thread.checkpoints.remove(victim);
            total = total.saturating_sub(size_of(&dropped.id));
            let _ = fs::remove_file(dir.join(format!("{}.json", dropped.id)));
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
        if provider_id != source.provider_id.as_deref() {
            fork.provider_kind = None;
        }
        // A fork owns a new provider conversation. Its canonical transcript is
        // copied, but a native continuation cursor must never be shared.
        fork.provider_session = None;
        // Pending work belongs to the source runtime. Copying it into a branch
        // would execute the same user request twice after a fork.
        fork.pending_inputs.clear();
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

    /// Permanently remove a thread file and everything derived from it. Missing
    /// files are success (idempotent).
    ///
    /// Side files are cleaned on the missing arm too, so re-deleting a thread
    /// whose JSON is already gone still collects its checkpoints and spilled tool
    /// output. Cleaning only on the removed arm left them behind for good.
    pub fn delete(&self, id: &str) -> Result<()> {
        let tid = ThreadId::parse(id)
            .map_err(|e| HarnessError::Other(format!("invalid thread id: {e}")))?;
        let path = self.path_for(&tid);
        let result = match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(HarnessError::Other(format!(
                "delete thread {}: {e}",
                path.display()
            ))),
        };
        if result.is_ok() {
            let _ = fs::remove_dir_all(self.checkpoints_dir_for(&tid));
            if let Some(zest_dir) = self.dir.parent() {
                crate::tools::spill::remove_thread_dir(zest_dir, &tid);
            }
        }
        result
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
            let Some(summary) = thread_summary_from_json(&body) else {
                continue;
            };
            if let Some(want) = provider_id {
                match summary.provider_id.as_deref() {
                    Some(id) if id == want => {}
                    _ => continue,
                }
            }
            out.push(summary);
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

    fn scratch(name: &str) -> crate::fsutil::ScratchDir {
        crate::fsutil::ScratchDir::new(&format!("zest-thread-{name}-"))
    }

    /// `exists` is the guard that stops a metadata-only write from creating a
    /// chat. It has to answer for an id that was never saved, and for one whose
    /// row has been deleted out from under a live session — that second case is
    /// exactly the draft the desktop holds after deleting the open chat.
    #[test]
    fn exists_reports_only_rows_actually_on_disk() {
        let root = scratch("exists");
        let store = ThreadStore::open(&root).unwrap();

        let unsaved = Thread::new();
        assert!(
            !store.exists(&unsaved.id),
            "a thread that was never saved has no row"
        );

        let saved = store.create().unwrap();
        assert!(store.exists(&saved.id));

        store.delete(&saved.id).unwrap();
        assert!(
            !store.exists(&saved.id),
            "a deleted chat must not look present to a live session still holding it"
        );

        assert!(!store.exists("not-a-thread-id"));
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
    fn listing_counts_messages_without_loading_agent_history() {
        let root = scratch("listing-skip-agent");
        let store = ThreadStore::open(&root).unwrap();
        let thread = Thread::new().with_provider("codex");
        let path = store.dir().join(format!("{}.json", thread.id));
        let agent_messages: Vec<serde_json::Value> = (0..40)
            .map(|i| serde_json::json!({"role": "assistant", "content": format!("blob-{i}")}))
            .collect();
        let body = serde_json::json!({
            "version": THREAD_FORMAT_VERSION,
            "id": thread.id,
            "createdAt": thread.created_at,
            "updatedAt": thread.updated_at,
            "providerId": "codex",
            "messages": [
                {"role": "user", "id": "u1", "text": "hello"}
            ],
            "pendingInputs": [
                {
                    "id": "p1",
                    "target": "followup",
                    "text": "later",
                    "createdAt": 1
                }
            ],
            "agentMessages": agent_messages,
            "events": [{"sequence": 1, "event": {"type": "turn_started", "turnId": "t1"}}],
            "checkpoints": [{"id": "c1"}],
            "gitContext": {"branch": "main"}
        });
        fs::write(&path, serde_json::to_vec(&body).unwrap()).unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, thread.id);
        assert_eq!(listed[0].message_count, 1);
        assert_eq!(listed[0].pending_input_count, 1);
        assert_eq!(
            listed[0].git_context.as_ref().unwrap().branch.as_deref(),
            Some("main")
        );

        let summary = thread_summary_from_json(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(summary.message_count, 1);
        assert_eq!(summary.pending_input_count, 1);
    }

    #[test]
    fn durable_input_queue_and_ledger_survive_restart_and_claim_in_order() {
        let root = scratch("durable-inputs");
        let store = ThreadStore::open(&root).unwrap();
        let mut thread = store.create_for_provider("codex").unwrap();
        let followup = thread
            .enqueue_input(ThreadInputTarget::Followup, "run the tests", Vec::new())
            .unwrap();
        let steer = thread
            .enqueue_input(
                ThreadInputTarget::Steer,
                "focus on the failing test",
                Vec::new(),
            )
            .unwrap();
        let inject = thread
            .enqueue_input(ThreadInputTarget::Inject, "job finished", Vec::new())
            .unwrap();
        store.save(&thread).unwrap();

        let mut restored = store.load(&thread.id).unwrap();
        assert_eq!(restored.pending_inputs.len(), 3);
        assert_eq!(restored.summary().pending_input_count, 3);
        assert!(restored.events.iter().any(|entry| matches!(
            entry.event,
            ThreadEventKind::InputQueued { ref input_id, target: ThreadInputTarget::Steer }
                if input_id == &steer.id
        )));

        let step_inputs = restored.claim_next_step_inputs();
        assert_eq!(
            step_inputs
                .iter()
                .map(|input| input.id.as_str())
                .collect::<Vec<_>>(),
            vec![steer.id.as_str(), inject.id.as_str()]
        );
        assert_eq!(restored.claim_followup().unwrap().id, followup.id);
        assert!(restored.pending_inputs.is_empty());
        assert_eq!(
            restored
                .events
                .iter()
                .filter(|entry| matches!(entry.event, ThreadEventKind::InputClaimed { .. }))
                .count(),
            3
        );
        assert!(restored
            .events
            .windows(2)
            .all(|entries| entries[0].sequence < entries[1].sequence));
    }

    #[test]
    fn v3_thread_migrates_without_inventing_pending_work() {
        let root = scratch("v3-migration");
        let store = ThreadStore::open(&root).unwrap();
        let thread = Thread::new().with_provider("codex");
        let path = store.dir().join(format!("{}.json", thread.id));
        let body = serde_json::json!({
            "version": 3,
            "id": thread.id,
            "createdAt": thread.created_at,
            "updatedAt": thread.updated_at,
            "providerId": "codex",
            "wireFormat": WIRE_FORMAT_ANTHROPIC_MESSAGES,
            "messages": [],
            "checkpoints": [],
            "agentMessages": []
        });
        fs::write(&path, serde_json::to_vec(&body).unwrap()).unwrap();

        let loaded = store.load_with_recovery(&thread.id).unwrap();
        assert_eq!(loaded.thread.version, THREAD_FORMAT_VERSION);
        assert!(loaded.thread.pending_inputs.is_empty());
        assert!(loaded.thread.events.is_empty());
        assert!(loaded
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("migrated thread from format v3")));
        let rewritten: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(rewritten["version"], THREAD_FORMAT_VERSION);
    }

    #[test]
    fn fork_drops_pending_runtime_inputs_but_keeps_transcript_ledger() {
        let root = scratch("fork-pending");
        let store = ThreadStore::open(&root).unwrap();
        let mut source = store.create_for_provider("codex").unwrap();
        source.apply_user("u1", "inspect the build");
        source.record_event(ThreadEventKind::TurnStarted {
            turn_id: "turn-1".into(),
        });
        source
            .enqueue_input(ThreadInputTarget::Followup, "then fix it", Vec::new())
            .unwrap();
        store.save(&source).unwrap();

        let fork = store.fork(&source, Some("branch")).unwrap();
        assert!(fork.pending_inputs.is_empty());
        assert_eq!(
            serde_json::to_value(&fork.messages).unwrap(),
            serde_json::to_value(&source.messages).unwrap()
        );
        assert_eq!(fork.events, source.events);
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

    /// A restore point somebody marked outlives the snapshots the harness takes
    /// on its own. Turn checkpoints land once per turn and compaction adds more,
    /// so without a preference a manual one is pushed out of the window inside a
    /// day's work.
    #[test]
    fn automatic_checkpoints_are_evicted_before_a_manual_one() {
        let root = scratch("checkpoint-manual");
        let store = ThreadStore::open(&root).unwrap();
        let mut thread = store.create_for_provider("codex").unwrap();
        thread.apply_user("u1", "a question");
        store.save(&thread).unwrap();

        let marked = store
            .create_checkpoint_with_metadata(
                &mut thread,
                "my restore point",
                None,
                None,
                ThreadCheckpointKind::Manual,
            )
            .unwrap();

        // Bury it under more automatic snapshots than the window can hold.
        for index in 0..ThreadStore::MAX_CHECKPOINTS + 4 {
            store
                .create_checkpoint(&mut thread, format!("turn {index}"))
                .unwrap();
        }

        assert!(thread.checkpoints.len() <= ThreadStore::MAX_CHECKPOINTS);
        assert!(
            thread.checkpoints.iter().any(|c| c.id == marked.id),
            "the manual checkpoint was evicted by automatic ones"
        );
        assert!(
            root.join(".zest/threads/checkpoints")
                .join(&thread.id)
                .join(format!("{}.json", marked.id))
                .exists(),
            "its snapshot file was deleted"
        );
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
                error,
                provider_selection,
                streaming,
                ..
            } => {
                assert_eq!(error.as_deref(), Some("upstream failed"));
                assert!(provider_selection.is_none());
                assert!(!streaming);
            }
            other => panic!("expected assistant, got {other:?}"),
        }

        thread.apply_error_with_provider("a1", "invalid key", Some("anthropic"));
        match &thread.messages[1] {
            StoredMessage::Assistant {
                error,
                provider_selection,
                ..
            } => {
                assert_eq!(error.as_deref(), Some("invalid key"));
                assert_eq!(provider_selection.as_deref(), Some("anthropic"));
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
    fn match_excerpt_windows_around_the_query() {
        let text = "Please git pull the latest OceanicUI component branch and open a pull request.";
        let snippet = match_excerpt(text, "git pu").expect("match");
        assert!(
            snippet.to_ascii_lowercase().contains("git pull"),
            "{snippet}"
        );
        assert!(snippet.contains('…'), "{snippet}");

        let mut thread = Thread::new();
        thread.apply_user("u1", text);
        let body = thread.search_excerpt("OceanicUI").expect("body");
        assert!(body.contains("OceanicUI"), "{body}");
        let (id, snippet) = thread.search_match("OceanicUI").expect("match");
        assert_eq!(id, "u1");
        assert_eq!(snippet, body);
        assert!(contains_ignore_ascii_case(text, "GIT PU"));
        assert!(!contains_ignore_ascii_case(text, "harness"));
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
    fn deleting_a_thread_removes_its_spilled_tool_output() {
        let root = scratch("delete-spill");
        let store = ThreadStore::open(&root).unwrap();
        let thread = store.create_for_provider("codex").unwrap();

        let spill = crate::tools::spill::SpillStore::open(&root, &thread.id).unwrap();
        let name = spill.next_name("grep");
        assert!(spill.write(&name, "a large result").is_some());
        let spill_dir = root.join(".zest/spill").join(&thread.id);
        assert!(spill_dir.is_dir());

        store.delete(&thread.id).unwrap();
        assert!(
            !spill_dir.exists(),
            "spilled output outlived its conversation"
        );
    }

    /// The thread JSON can already be gone — a half-finished delete, or a second
    /// call — and its side files still have to be collected.
    #[test]
    fn deleting_a_missing_thread_still_collects_its_side_files() {
        let root = scratch("delete-missing");
        let store = ThreadStore::open(&root).unwrap();
        let thread = store.create_for_provider("codex").unwrap();

        let spill = crate::tools::spill::SpillStore::open(&root, &thread.id).unwrap();
        assert!(spill.write(&spill.next_name("grep"), "orphan").is_some());
        let spill_dir = root.join(".zest/spill").join(&thread.id);

        // Remove only the thread row, leaving the derived files behind.
        fs::remove_file(store.dir().join(format!("{}.json", thread.id))).unwrap();
        assert!(spill_dir.is_dir());

        store.delete(&thread.id).unwrap();
        assert!(!spill_dir.exists(), "{}", spill_dir.display());
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

    #[test]
    fn a_codex_oauth_thread_does_not_resume_on_codex_cli() {
        let oauth = Thread::new()
            .with_provider("codex")
            .with_provider_kind("codex_oauth");
        assert!(oauth.assert_provider_kind("codex_oauth").is_ok());
        assert!(matches!(
            oauth.assert_provider_kind("codex_cli"),
            Err(ThreadLoadError::ProviderKindMismatch { .. })
        ));

        let legacy = Thread::new().with_provider("codex");
        assert!(legacy.assert_provider_kind("codex_cli").is_ok());
        assert!(matches!(
            legacy.assert_provider_kind("codex_oauth"),
            Err(ThreadLoadError::ProviderKindMismatch { .. })
        ));
        assert!(legacy
            .assert_provider_kind("codex_oauth")
            .unwrap_err()
            .to_string()
            .contains("different Codex sign-in"));
    }

    #[test]
    fn reassigning_a_chat_clears_the_native_cursor() {
        let mut thread = Thread::new()
            .with_provider("codex")
            .with_provider_kind("codex_oauth");
        thread.provider_session = Some(crate::ProviderSessionRef::CodexAppServer {
            thread_id: "native-1".into(),
        });
        thread.reassign_provider("deepseek", Some("openai_compatible"));
        assert_eq!(thread.provider_id.as_deref(), Some("deepseek"));
        assert_eq!(thread.provider_kind.as_deref(), Some("openai_compatible"));
        assert_eq!(thread.provider_session, None);
    }

    #[test]
    fn fork_for_provider_keeps_the_transcript_and_leaves_the_source() {
        let mut thread = Thread::new()
            .with_provider("codex")
            .with_provider_kind("codex_cli");
        thread.apply_user("user-1", "hello");
        thread.title = Some("Native chat".into());
        thread.provider_session = Some(crate::ProviderSessionRef::CodexAppServer {
            thread_id: "native-1".into(),
        });
        let copy = thread.fork_for_provider("deepseek", Some("openai_compatible"));
        assert_ne!(copy.id, thread.id);
        assert_eq!(copy.provider_id.as_deref(), Some("deepseek"));
        assert_eq!(copy.provider_kind.as_deref(), Some("openai_compatible"));
        assert_eq!(copy.provider_session, None);
        assert_eq!(copy.title.as_deref(), Some("Copy of Native chat"));
        assert_eq!(copy.messages.len(), 1);
        assert_eq!(thread.provider_id.as_deref(), Some("codex"));
        assert_eq!(
            thread.provider_session,
            Some(crate::ProviderSessionRef::CodexAppServer {
                thread_id: "native-1".into(),
            })
        );
    }
}
