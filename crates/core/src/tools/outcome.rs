//! Tool results visible to the model, plus optional typed UI metadata.
//!
//! The model only ever sees [`ToolOutcome::body`]. Front-ends may also receive
//! [`ToolMetadata`] (external-worker provenance) without stuffing
//! structured JSON into the wire `tool_result`.

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
}

impl ToolOutcome {
    pub fn text(body: impl Into<String>) -> Self {
        Self {
            body: body.into(),
            metadata: None,
        }
    }

    pub fn with_metadata(body: impl Into<String>, metadata: ToolMetadata) -> Self {
        Self {
            body: body.into(),
            metadata: Some(metadata),
        }
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
        usage: Option<crate::usage::ExternalUsageReport>,
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
        }
    }

    pub fn delegation_diff(&self) -> Option<&str> {
        match self {
            Self::Delegation { diff, .. } => diff.as_deref(),
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
            usage: Some(crate::usage::ExternalUsageReport {
                input_tokens: Some(12),
                ..Default::default()
            }),
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
        }
    }
}
