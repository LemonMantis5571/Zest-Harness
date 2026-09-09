//! Coordinator-only feature-card creation tool.
//!
//! The tool owns validation and durable card creation. After the parent
//! approval gate, it writes a dispatch receipt next to the card so any live
//! coordinator can enqueue the job. Implementation still does not start until
//! that receipt is consumed.

use std::collections::BTreeMap;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::approval::{ApprovalPreview, ToolRisk};
use super::outcome::{ToolMetadata, ToolOutcome};
use super::prepared::PreparedToolCall;
use super::Tool;
use crate::config::ExternalAgentConfig;
use crate::delegation::{
    DelegationStore, FeatureCard, DELEGATION_FORMAT_VERSION, MAX_FEATURE_CHECKS,
    MAX_FEATURE_CONTEXT_CHARS, MAX_FEATURE_DEPENDENCIES, MAX_FEATURE_LANE_CHARS,
    MAX_FEATURE_OBJECTIVE_CHARS, MAX_FEATURE_PATHS, MAX_FEATURE_TITLE_CHARS,
};
use crate::thread::new_id;

pub const DELEGATE_FEATURE_TOOL: &str = "delegate_feature";

pub struct FeatureDelegator {
    root: PathBuf,
    agents: BTreeMap<String, ExternalAgentConfig>,
    parent_thread_id: String,
}

impl FeatureDelegator {
    pub fn new(
        root: impl Into<PathBuf>,
        agents: BTreeMap<String, ExternalAgentConfig>,
        parent_thread_id: impl Into<String>,
    ) -> Self {
        Self {
            root: root.into(),
            agents,
            parent_thread_id: parent_thread_id.into(),
        }
    }

    fn parse(&self, input: &Value) -> Result<FeatureCard, String> {
        let text = |name: &str, max: usize| -> Result<String, String> {
            let value = input
                .get(name)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("missing required field {name}"))?
                .trim()
                .to_string();
            if value.is_empty() {
                return Err(format!("feature card {name} must not be empty"));
            }
            if value.chars().count() > max {
                return Err(format!("feature card {name} exceeds {max} characters"));
            }
            Ok(value)
        };
        let list = |name: &str, max: usize| -> Result<Vec<String>, String> {
            let values = input
                .get(name)
                .and_then(Value::as_array)
                .ok_or_else(|| format!("missing required field {name}"))?;
            if values.len() > max {
                return Err(format!("feature card {name} has too many entries"));
            }
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(|text| text.trim().to_string())
                        .filter(|text| !text.is_empty())
                        .ok_or_else(|| format!("feature card {name} contains an empty entry"))
                })
                .collect()
        };

        let title = text("title", MAX_FEATURE_TITLE_CHARS)?;
        let objective = text("objective", MAX_FEATURE_OBJECTIVE_CHARS)?;
        let lane = text("lane", MAX_FEATURE_LANE_CHARS)?;
        let scope = list("scope", MAX_FEATURE_PATHS)?;
        if scope.is_empty() {
            return Err("feature card scope must not be empty".into());
        }
        let context = list("context", MAX_FEATURE_PATHS)?;
        let acceptance_checks = list("acceptance_checks", MAX_FEATURE_CHECKS)?;
        let depends_on = list("depends_on", MAX_FEATURE_DEPENDENCIES)?;
        let agent = text("agent", 120)?;
        let review_required = input
            .get("review_required")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let card = FeatureCard {
            version: DELEGATION_FORMAT_VERSION,
            card_id: new_id("card"),
            title,
            objective,
            lane,
            scope,
            context,
            depends_on,
            agent,
            worker_target: None,
            acceptance_checks,
            review_required,
            reviewer_target: crate::delegation::ReviewerTarget::SameAsWorker,
            created_at: 0,
        };
        card.validate(&self.root, &self.agents)
            .map_err(|error| error.to_string())?;
        let context_chars: usize = card.context.iter().map(|path| path.chars().count()).sum();
        if context_chars > MAX_FEATURE_CONTEXT_CHARS {
            return Err("feature card context selection is too large".into());
        }
        Ok(card)
    }

    fn agent_target(&self, card: &FeatureCard) -> Result<(String, String), String> {
        let config = self
            .agents
            .get(&card.agent)
            .ok_or_else(|| format!("external agent {} is not configured", card.agent))?;
        let model = config
            .model
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("CLI default");
        Ok((
            format!("agent/{}/feature/{model}", card.agent),
            model.to_string(),
        ))
    }
}

#[async_trait]
impl Tool for FeatureDelegator {
    fn name(&self) -> &str {
        DELEGATE_FEATURE_TOOL
    }

    fn description(&self) -> &str {
        "Create a bounded feature card for an isolated external worker and its independent reviewer. The card enters the coordinator board; implementation does not start until the approval gate is satisfied."
    }

    fn input_schema(&self) -> Value {
        let agents: Vec<&String> = self.agents.keys().collect();
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Short feature-card title."},
                "objective": {"type": "string", "description": "Concrete outcome for the worker."},
                "lane": {"type": "string", "description": "Build lane or area."},
                "scope": {"type": "array", "items": {"type": "string"}, "description": "Relative project paths the worker may change."},
                "context": {"type": "array", "items": {"type": "string"}, "description": "Relative files to include as focused context."},
                "acceptance_checks": {"type": "array", "items": {"type": "string"}, "description": "Commands the reviewer must run and evidence."},
                "agent": {"type": "string", "enum": agents, "description": "Configured isolated external worker."},
                "depends_on": {"type": "array", "items": {"type": "string"}, "description": "Earlier feature-card job ids that must be accepted first."},
                "review_required": {"type": "boolean", "default": true}
            },
            "required": ["title", "objective", "lane", "scope", "context", "acceptance_checks", "agent", "depends_on"],
            "additionalProperties": false
        })
    }

    fn risk(&self) -> ToolRisk {
        ToolRisk::Exec
    }

    async fn execute_prepared(
        &self,
        prepared: PreparedToolCall,
    ) -> std::result::Result<ToolOutcome, String> {
        let approved = prepared.preview.path.clone();
        let input = prepared
            .plain_input()
            .cloned()
            .ok_or_else(|| "internal error: feature-card prepared call mismatch".to_string())?;
        let card = self.parse(&input)?;
        let (target, _) = self.agent_target(&card)?;
        if approved != target {
            return Err(format!(
                "external agent configuration changed after approval ({approved} -> {target}); aborting; fresh approval required"
            ));
        }
        self.run(input).await
    }

    fn prepare(&self, input: Value) -> Result<PreparedToolCall, String> {
        let card = self.parse(&input)?;
        let (target, model) = self.agent_target(&card)?;
        let summary = format!(
            "Create reviewed feature card for {} ({model}) in isolated workspace",
            card.agent
        );
        Ok(PreparedToolCall::plain_with_preview(
            DELEGATE_FEATURE_TOOL,
            ToolRisk::Exec,
            input,
            ApprovalPreview {
                path: target,
                summary,
                diff: String::new(),
            },
        )
        .with_metadata(ToolMetadata::Delegation {
            provider_id: card.agent,
            model,
            diff: None,
            usage: None,
            job_id: None,
            stage: Some("card_approval".into()),
            attempt: None,
            review_status: Some("pending".into()),
        }))
    }

    async fn run(&self, input: Value) -> std::result::Result<ToolOutcome, String> {
        let mut card = self.parse(&input)?;
        card.created_at = crate::delegation::capture_workspace_snapshot(&self.root).captured_at;
        let store = DelegationStore::open(&self.root).map_err(|error| error.to_string())?;
        let existing = store.list().map_err(|error| error.to_string())?;
        for dependency in &card.depends_on {
            if !existing.iter().any(|job| &job.job_id == dependency) {
                return Err(format!(
                    "feature card dependency {dependency} does not exist"
                ));
            }
        }
        let mut job = store
            .create(
                &self.parent_thread_id,
                card.clone(),
                crate::delegation::capture_workspace_snapshot(&self.root),
            )
            .map_err(|error| error.to_string())?;
        job.origin = Some(crate::delegation::DelegationOrigin {
            coordinator: "interactive_tool".into(),
            chat_id: None,
            thread_id: Some(self.parent_thread_id.clone()),
            idempotency_key: None,
        });
        job.grant_dispatch_receipt("delegate_feature");
        let job = store.update(job).map_err(|error| error.to_string())?;
        let model = self
            .agents
            .get(&card.agent)
            .and_then(|config| config.model.clone())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "CLI default".into());
        Ok(ToolOutcome::with_metadata(
            format!(
                "Feature card `{}` created in lane `{}` and is waiting for the coordinator to pick up the recorded approval.",
                job.job_id, card.lane
            ),
            ToolMetadata::Delegation {
                provider_id: card.agent,
                model,
                diff: None,
                usage: None,
                job_id: Some(job.job_id),
                stage: Some("card_created".into()),
                attempt: Some(job.attempt),
                review_status: Some("pending".into()),
            },
        ))
    }
}
