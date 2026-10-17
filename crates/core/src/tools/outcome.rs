//! Tool results visible to the model, plus optional typed UI metadata.
//!
//! The model sees body text and explicit screenshot pixels. UI metadata stays
//! separate from wire content and content-free usage traces.

use serde::{Deserialize, Serialize};

/// What a tool returns after execution.
#[derive(Debug, Clone)]
pub struct ToolOutcome {
    /// Model-visible result string (also summarized for the UI when metadata
    /// does not replace the card copy).
    pub body: String,
    /// Optional typed side-channel for the UI / persistence. Never sent on the
    /// Messages API wire as structured content.
    pub metadata: Option<ToolMetadata>,
    /// Validated screenshot pixels, separate from UI metadata and text spilling.
    pub images: Vec<ToolImage>,
}

impl ToolOutcome {
    pub fn text(body: impl Into<String>) -> Self {
        Self {
            body: body.into(),
            metadata: None,
            images: Vec::new(),
        }
    }

    pub fn with_metadata(body: impl Into<String>, metadata: ToolMetadata) -> Self {
        Self {
            body: body.into(),
            metadata: Some(metadata),
            images: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolImage {
    png_base64: String,
}

impl ToolImage {
    pub fn png(png_base64: String) -> Result<Self, String> {
        use base64::Engine;
        if png_base64.len() > 2 * 1024 * 1024 * 4 / 3 + 4 {
            return Err("screenshot exceeds 2 MiB".into());
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&png_base64)
            .map_err(|_| "invalid screenshot encoding")?;
        if bytes.len() > 2 * 1024 * 1024 {
            return Err("screenshot exceeds 2 MiB".into());
        }
        if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.len() < 24 {
            return Err("screenshot is not a PNG".into());
        }
        let width = u32::from_be_bytes(bytes[16..20].try_into().map_err(|_| "invalid PNG")?);
        let height = u32::from_be_bytes(bytes[20..24].try_into().map_err(|_| "invalid PNG")?);
        if width == 0 || height == 0 || width > 1920 || height > 1920 {
            return Err("screenshot dimensions are out of bounds".into());
        }
        Ok(Self { png_base64 })
    }

    pub fn content_block(&self) -> serde_json::Value {
        serde_json::json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":self.png_base64}})
    }
}

impl From<String> for ToolOutcome {
    fn from(body: String) -> Self {
        Self::text(body)
    }
}

impl From<&str> for ToolOutcome {
    fn from(body: &str) -> Self {
        Self::text(body)
    }
}

/// Typed tool side-channel. Extend with new variants; unknown variants must not
/// break older UIs (serde will fail closed on load — prefer additive fields).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolMetadata {
    HtmlDocument {
        title: String,
        html: String,
    },
    Delegation {
        provider_id: String,
        model: String,
        /// Optional worker diff for front-ends that can open a review view.
        /// The model-visible answer remains in `ToolOutcome::body`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff: Option<String>,
        /// Optional usage volunteered by the external worker. This is a
        /// runtime side-channel for the ledger and front-end event; it is not
        /// written into thread history or sent to the model.
        #[serde(skip)]
        usage: Option<Box<crate::usage::ExternalUsageReport>>,
        /// Additive orchestration identity. Older direct delegation metadata
        /// omits these fields and remains valid through serde defaults.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        job_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stage: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        review_status: Option<String>,
    },
}

/// Heading an external worker's body puts before the diff of its workspace.
pub(crate) const WORKSPACE_CHANGES_HEADING: &str = "\n\nChanges from the external workspace:\n";

/// Longest worker report the UI row carries. The model still gets the whole
/// body; this only bounds what every saved thread repeats.
const DELEGATION_REPORT_CHARS: usize = 600;

impl ToolMetadata {
    /// The row summary for a delegated call: what the worker reported.
    ///
    /// An external worker's body opens with an `[agent · model]` line, which the
    /// row already shows as a badge, and may end with its diff, which the row
    /// opens in the diff viewer; neither is part of the report. An empty report
    /// falls back to naming who the work went to.
    pub fn delegation_summary(&self, body: &str) -> Option<String> {
        let Self::Delegation {
            provider_id, model, ..
        } = self
        else {
            return None;
        };
        let report = body.split(WORKSPACE_CHANGES_HEADING).next().unwrap_or(body);
        let report = match report.split_once('\n') {
            Some((first, rest)) if first.starts_with('[') && first.ends_with(']') => rest,
            _ => report,
        }
        .trim();
        if report.is_empty() {
            return Some(format!("Delegated to {provider_id} · {model}"));
        }
        if report.chars().count() <= DELEGATION_REPORT_CHARS {
            return Some(report.to_string());
        }
        let cut: String = report.chars().take(DELEGATION_REPORT_CHARS - 1).collect();
        Some(format!("{}…", cut.trim_end()))
    }

    pub fn delegation_diff(&self) -> Option<&str> {
        match self {
            Self::Delegation { diff, .. } => diff.as_deref(),
            Self::HtmlDocument { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_usage_side_channel_is_not_serialized_into_thread_metadata() {
        let metadata = ToolMetadata::Delegation {
            provider_id: "claude".into(),
            model: "sonnet".into(),
            diff: None,
            usage: Some(Box::new(crate::usage::ExternalUsageReport {
                input_tokens: Some(12),
                ..Default::default()
            })),
            job_id: None,
            stage: None,
            attempt: None,
            review_status: None,
        };
        let value = serde_json::to_value(&metadata).unwrap();
        assert_eq!(value["kind"], "delegation");
        assert!(value.get("usage").is_none());

        let restored: ToolMetadata = serde_json::from_value(value).unwrap();
        match restored {
            ToolMetadata::Delegation { usage, .. } => assert!(usage.is_none()),
            _ => panic!("expected delegation metadata"),
        }
    }

    fn delegation() -> ToolMetadata {
        ToolMetadata::Delegation {
            provider_id: "codex".into(),
            model: "gpt-5".into(),
            diff: None,
            usage: None,
            job_id: None,
            stage: None,
            attempt: None,
            review_status: None,
        }
    }

    /// The subagent line used to say only "Delegated to codex · gpt-5", so the
    /// user could not see what the worker found without asking the model.
    #[test]
    fn delegation_summary_is_the_worker_report_without_header_or_diff() {
        let body = format!(
            "[codex · gpt-5]\nThe cache key ignores the locale.\nKey it on both.{WORKSPACE_CHANGES_HEADING}--- a/x\n+++ b/x\n"
        );
        assert_eq!(
            delegation().delegation_summary(&body).as_deref(),
            Some("The cache key ignores the locale.\nKey it on both.")
        );
        assert_eq!(
            delegation()
                .delegation_summary("[codex · gpt-5]\n  ")
                .as_deref(),
            Some("Delegated to codex · gpt-5")
        );
        let long = "x".repeat(DELEGATION_REPORT_CHARS * 2);
        let summary = delegation().delegation_summary(&long).unwrap();
        assert_eq!(summary.chars().count(), DELEGATION_REPORT_CHARS);
        assert!(summary.ends_with('…'));
        assert!(ToolMetadata::HtmlDocument {
            title: "t".into(),
            html: "<p></p>".into()
        }
        .delegation_summary("body")
        .is_none());
    }
}
