//! Claude Code as a first-class parent provider.
//!
//! Claude Code owns the subscription session and its own model/tool loop. Zest
//! supplies the project root and the parent conversation, then treats the CLI's
//! completed answer as one provider completion. This is intentionally separate
//! from the delegate_external worker path: no second Zest agent is created and
//! no isolated worktree or diff is introduced.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{catalogue_without_efforts, Completion, ModelSpec, Provider, StreamEvent, TurnRequest};
use crate::anthropic::types::Usage;
use crate::auth::{detect_claude_code, AuthStatus};
use crate::config::{
    ClaudeCodePermissionMode, ExternalAgentConfig, ExternalAgentMode, ExternalWorkspace,
    DEFAULT_CLAUDE_CODE_MODEL,
};
use crate::error::{HarnessError, Result};
use crate::tools::external_agent::{run_headless_command_streaming, ExternalAgentEvent};

const BUILTIN_MODELS: &[&str] = &["sonnet", "opus", "haiku"];

pub struct ClaudeCodeProvider {
    id: String,
    root: PathBuf,
    command: String,
    default_model: String,
    models: Vec<ModelSpec>,
    allow_mcp: bool,
    permission_mode: ClaudeCodePermissionMode,
    timeout_secs: u64,
    auth: AuthStatus,
}

impl ClaudeCodeProvider {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        root: impl Into<PathBuf>,
        command: impl Into<String>,
        model: Option<String>,
        models: Vec<String>,
        allow_mcp: bool,
        permission_mode: ClaudeCodePermissionMode,
        timeout_secs: u64,
    ) -> Result<Self> {
        let command = command.into();
        if command.trim().is_empty() {
            return Err(HarnessError::Other(
                "Claude Code command cannot be empty".into(),
            ));
        }
        if timeout_secs == 0 || timeout_secs > 3_600 {
            return Err(HarnessError::Other(
                "Claude Code timeout_secs must be between 1 and 3600".into(),
            ));
        }

        let default_model = model
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_CLAUDE_CODE_MODEL.to_string());
        let models = if models.is_empty() {
            BUILTIN_MODELS
                .iter()
                .map(|value| (*value).to_string())
                .collect()
        } else {
            models
        };

        Ok(Self {
            id: id.into(),
            root: root.into(),
            command,
            default_model: default_model.clone(),
            models: catalogue_without_efforts(&default_model, &models),
            allow_mcp,
            permission_mode,
            timeout_secs,
            auth: detect_claude_code(),
        })
    }

    fn config_for(&self, model: &str) -> ExternalAgentConfig {
        ExternalAgentConfig {
            mode: ExternalAgentMode::Headless,
            command: self.command.clone(),
            args: vec![
                "--print".into(),
                "--output-format".into(),
                "stream-json".into(),
                "--include-partial-messages".into(),
                "--permission-mode".into(),
                self.permission_mode.cli_value().into(),
                "--model".into(),
                "{model}".into(),
                "{prompt}".into(),
            ],
            allow_mcp: self.allow_mcp,
            model: Some(model.to_string()),
            workspace: ExternalWorkspace::Current,
            timeout_secs: self.timeout_secs,
        }
    }
}

#[async_trait]
impl Provider for ClaudeCodeProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn models(&self) -> Vec<ModelSpec> {
        self.models.clone()
    }

    fn auth_status(&self) -> AuthStatus {
        self.auth.clone()
    }

    fn owns_agent_loop(&self) -> bool {
        true
    }

    async fn stream_turn(
        &self,
        req: &TurnRequest,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    ) -> Result<Completion> {
        if req
            .cancel
            .as_ref()
            .is_some_and(|cancel| cancel.is_cancelled())
        {
            return Err(HarnessError::Cancelled);
        }

        let prompt = parent_prompt(req);
        let config = self.config_for(&req.model);
        let mut streamed_text = false;
        let run = {
            let mut on_external_event = |event: ExternalAgentEvent| match event {
                ExternalAgentEvent::TextDelta(text) => {
                    streamed_text = true;
                    on_event(StreamEvent::Text(&text));
                }
                ExternalAgentEvent::Thinking(text) => {
                    on_event(StreamEvent::Thinking(&text));
                }
                ExternalAgentEvent::ToolCall { id, title, status } => {
                    on_event(StreamEvent::ProviderActivity {
                        id: &id,
                        title: &title,
                        status: &status,
                    });
                }
                _ => {}
            };
            run_headless_command_streaming(
                &self.root,
                &config,
                &prompt,
                req.cancel.as_ref(),
                &mut on_external_event,
            )
            .await?
        };

        if req
            .cancel
            .as_ref()
            .is_some_and(|cancel| cancel.is_cancelled())
        {
            return Err(HarnessError::Cancelled);
        }

        let answer = run.text();
        if answer.trim().is_empty() {
            if let Some(error) = run.errors().first() {
                return Err(HarnessError::Other(format!(
                    "Claude Code parent returned an error: {error}"
                )));
            }
            return Err(HarnessError::Other(
                "Claude Code parent returned no answer".into(),
            ));
        }

        // Claude's partial events have already reached the UI. The completed
        // answer is still retained for history, but emitting it here would
        // duplicate every streamed character in the transcript.
        if !streamed_text && !run.has_streamed_text() {
            on_event(StreamEvent::Text(&answer));
        }

        let reported = run.usage();
        let usage_available = reported.as_ref().is_some_and(|usage| usage.has_tokens());
        let usage = reported
            .as_ref()
            .map(|usage| Usage {
                input_tokens: bounded_u32(usage.input_tokens),
                output_tokens: bounded_u32(usage.output_tokens),
                cache_creation_input_tokens: bounded_u32(usage.cached_write_tokens),
                cache_read_input_tokens: bounded_u32(usage.cached_read_tokens),
            })
            .unwrap_or_default();

        Ok(Completion {
            content: vec![json!({"type": "text", "text": answer})],
            stop_reason: Some("end_turn".into()),
            usage,
            usage_available,
            limits: run.limits(),
            served_model: None,
            provider_session: None,
        })
    }
}

fn bounded_u32(value: Option<u64>) -> u32 {
    value.unwrap_or_default().min(u64::from(u32::MAX)) as u32
}

fn parent_prompt(req: &TurnRequest) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "You are the parent coding agent running inside Zest through an authenticated \
         provider runtime. Work directly in the active project. Do not delegate this \
         request to another agent. Use the project instructions and the tools available \
         in this provider session.\n\n",
    );

    if let Some(system) = req
        .system
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        prompt.push_str("# Zest operating context\n\n");
        prompt.push_str(system);
        prompt.push_str("\n\n");
    }

    prompt.push_str("# Conversation\n");
    for message in &req.messages {
        let role = match message.role.as_str() {
            "assistant" => "Assistant",
            "user" => "User",
            other => other,
        };
        prompt.push('\n');
        prompt.push_str(role);
        prompt.push_str(":\n");
        let text = render_content(&message.content);
        if text.is_empty() {
            prompt.push_str("[non-text content]\n");
        } else {
            prompt.push_str(&text);
            prompt.push('\n');
        }
    }
    prompt.push_str(
        "\nContinue from the conversation above and complete the latest user request. \
         Report the result clearly when the work is finished.",
    );
    prompt
}

fn render_content(content: &[Value]) -> String {
    let mut output = String::new();
    for block in content {
        if let Some(text) = text_value(block) {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&text);
            continue;
        }
        if block.get("type").and_then(Value::as_str) == Some("tool_use") {
            let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
            output.push_str(&format!("[Zest tool call: {name}]\n"));
        }
    }
    output
}

fn text_value(value: &Value) -> Option<String> {
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    if let Some(items) = value.as_array() {
        let mut output = String::new();
        for item in items {
            if let Some(text) = text_value(item) {
                output.push_str(&text);
            }
        }
        return (!output.is_empty()).then_some(output);
    }
    value
        .get("content")
        .and_then(text_value)
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::types::Message;

    #[test]
    fn default_model_and_aliases_are_available() {
        let provider = ClaudeCodeProvider::new(
            "claude",
            ".",
            "claude",
            None,
            Vec::new(),
            false,
            ClaudeCodePermissionMode::AcceptEdits,
            900,
        )
        .unwrap();

        assert_eq!(provider.default_model(), "sonnet");
        assert!(provider.models().iter().any(|model| model.id == "opus"));
        assert!(provider.owns_agent_loop());
    }

    #[test]
    fn config_requests_live_claude_stream_events() {
        let provider = ClaudeCodeProvider::new(
            "claude",
            ".",
            "claude",
            None,
            Vec::new(),
            false,
            ClaudeCodePermissionMode::AcceptEdits,
            900,
        )
        .unwrap();

        assert!(provider
            .config_for("sonnet")
            .args
            .iter()
            .any(|arg| arg == "--include-partial-messages"));
    }

    #[test]
    fn parent_prompt_keeps_system_and_latest_conversation() {
        let request = TurnRequest {
            model: "sonnet".into(),
            system: Some("Follow the project rules.".into()),
            messages: vec![
                Message::user_text("Inspect the loader."),
                Message::assistant(vec![json!({"type": "text", "text": "I will inspect it."})]),
                Message::user_text("Now implement the fix."),
            ],
            tools: Vec::new(),
            max_tokens: 100,
            effort: None,
            thinking: false,
            provider_session: None,
            interaction: None,
            cancel: None,
        };

        let prompt = parent_prompt(&request);
        assert!(prompt.contains("Follow the project rules."));
        assert!(prompt.contains("Inspect the loader."));
        assert!(prompt.contains("Now implement the fix."));
        assert!(prompt.contains("Do not delegate"));
    }
}
