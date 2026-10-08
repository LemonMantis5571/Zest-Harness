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

impl ToolMetadata {
    pub fn delegation_label(&self) -> Option<String> {
        match self {
            Self::Delegation {
                provider_id, model, ..
            } => Some(format!("Delegated to {provider_id} · {model}")),
            Self::HtmlDocument { .. } => None,
        }
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
}
