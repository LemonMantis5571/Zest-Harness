//! Prepared tool calls — built once before approval, then executed.
//!
//! Write tools bind a normalized path and a BLAKE3 pre-image so a stale
//! approval cannot silently overwrite a file the user never saw.

use std::path::PathBuf;

use serde_json::Value;

use super::approval::{ApprovalPreview, ToolRisk};
use super::outcome::ToolMetadata;

/// Fingerprint of the on-disk bytes at prepare time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreImage {
    /// Target did not exist when prepared.
    Absent,
    /// Target existed and hashed to this digest.
    Present { blake3: [u8; 32] },
}

impl PreImage {
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self::Present {
            blake3: *blake3::hash(bytes).as_bytes(),
        }
    }

    pub fn digest_hex(&self) -> String {
        match self {
            Self::Absent => "absent".into(),
            Self::Present { blake3 } => hex_blake3(blake3),
        }
    }
}

fn hex_blake3(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// A tool invocation snapshotted before the approval gate.
#[derive(Debug, Clone)]
pub struct PreparedToolCall {
    pub tool_name: String,
    pub risk: ToolRisk,
    pub preview: ApprovalPreview,
    /// UI-only provenance known once the call's full input has been prepared.
    /// This is never sent to the provider or persisted as tool input.
    pub metadata: Option<ToolMetadata>,
    /// The tool vouches for *this* invocation as read-only.
    ///
    /// Only `bash` sets it, for a command that matches the allowlist and holds
    /// no shell metacharacters. It is an input to the mode policy, never a
    /// bypass: Manual mode still asks, and Plan mode still refuses.
    pub auto_eligible: bool,
    pub(crate) kind: PreparedKind,
}

#[derive(Debug, Clone)]
pub(crate) enum PreparedKind {
    /// Ordinary tool: execute with this input after optional approval.
    Plain { input: Value },
    /// A whole-file replacement bound to path + pre-image. Shared by
    /// `write_file` and `edit_file` — an edit is computed up front, so by the
    /// time it reaches the approval gate it is the same kind of commitment.
    WriteFile {
        absolute_path: PathBuf,
        relative_path: String,
        content: String,
        preimage: PreImage,
    },
}

impl PreparedToolCall {
    pub fn plain(tool_name: impl Into<String>, risk: ToolRisk, input: Value) -> Self {
        let tool_name = tool_name.into();
        Self {
            tool_name: tool_name.clone(),
            risk,
            preview: ApprovalPreview {
                path: String::new(),
                summary: format!("{tool_name} requires approval"),
                diff: String::new(),
            },
            metadata: None,
            auto_eligible: false,
            kind: PreparedKind::Plain { input },
        }
    }

    pub fn plain_with_preview(
        tool_name: impl Into<String>,
        risk: ToolRisk,
        input: Value,
        preview: ApprovalPreview,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            risk,
            preview,
            metadata: None,
            auto_eligible: false,
            kind: PreparedKind::Plain { input },
        }
    }

    /// Mark this specific invocation as read-only, for the mode policy to use.
    pub fn auto_eligible(mut self, eligible: bool) -> Self {
        self.auto_eligible = eligible;
        self
    }

    pub fn with_metadata(mut self, metadata: ToolMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// A prepared whole-file replacement. `tool_name` decides which tool the
    /// registry dispatches execution back to, so `edit_file` keeps its own
    /// identity in the UI while sharing the write path.
    pub fn write_kind(
        tool_name: impl Into<String>,
        absolute_path: PathBuf,
        relative_path: String,
        content: String,
        preimage: PreImage,
        preview: ApprovalPreview,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            risk: ToolRisk::Write,
            preview,
            metadata: None,
            auto_eligible: false,
            kind: PreparedKind::WriteFile {
                absolute_path,
                relative_path,
                content,
                preimage,
            },
        }
    }

    pub fn write_file(
        absolute_path: PathBuf,
        relative_path: String,
        content: String,
        preimage: PreImage,
        preview: ApprovalPreview,
    ) -> Self {
        Self::write_kind(
            "write_file",
            absolute_path,
            relative_path,
            content,
            preimage,
            preview,
        )
    }

    pub fn plain_input(&self) -> Option<&Value> {
        match &self.kind {
            PreparedKind::Plain { input } => Some(input),
            PreparedKind::WriteFile { .. } => None,
        }
    }
}
