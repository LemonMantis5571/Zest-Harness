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

use super::claude_control::{
    control_response, decide, initialize_request_id, initialize_response, render_diff,
    stream_json_user_message, summarize, surface_for, Surface, ToolPermissionRequest,
};
use super::{
    catalogue, Completion, EffortPolicy, ModelSpec, Provider, StreamEvent, SystemPrompt,
    TurnRequest,
};
use crate::anthropic::types::Usage;
use crate::auth::{detect_claude_code, AuthStatus};
use crate::config::{
    ClaudeCodePermissionMode, ExternalAgentConfig, ExternalAgentMode, ExternalWorkspace,
    DEFAULT_CLAUDE_CODE_MODEL,
};
use crate::error::{HarnessError, Result};
use crate::thread::new_id;
use crate::tools::approval::ToolRisk;
use crate::tools::external_agent::{
    run_headless_command_streaming, ControlResponder, ExternalAgentEvent,
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

/// Aliases the Claude Code CLI accepts with no configuration.
///
/// `pub(crate)` so the driver builds the picker catalogue from the same list the
/// provider accepts. It was private, and the driver passed an empty builtin list
/// instead: an entry with no `models` offered `[sonnet]` in the picker while the
/// provider accepted `[sonnet, opus, haiku]`.
pub(crate) const BUILTIN_MODELS: &[&str] = &["sonnet", "opus", "haiku"];

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
            models: catalogue(&default_model, &models, &[], EffortPolicy::Unsupported),
            allow_mcp,
            permission_mode,
            timeout_secs,
            auth: detect_claude_code(),
        })
    }

    /// The mode the CLI actually runs in.
    ///
    /// `AcceptEdits` and `BypassPermissions` auto-approve *before* the callback
    /// is consulted, so either would silently defeat the approval card. Both
    /// become `Default` — the mode that actually asks.
    ///
    /// This does not depend on whether a front-end is attached. Who gets asked
    /// is the responder's business: a host renders a card, and no host denies.
    /// The mode only has to guarantee the CLI asks at all.
    fn effective_permission_mode(&self) -> ClaudeCodePermissionMode {
        match self.permission_mode {
            ClaudeCodePermissionMode::Plan => ClaudeCodePermissionMode::Plan,
            _ => ClaudeCodePermissionMode::Default,
        }
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
                // Bidirectional stream-json plus `stdio` is what routes the
                // CLI's permission prompts to us instead of letting it decide
                // locally. Without the flag it silently denies whatever it
                // cannot auto-approve.
                "--input-format".into(),
                "stream-json".into(),
                "--permission-prompt-tool".into(),
                "stdio".into(),
                "--permission-mode".into(),
                self.effective_permission_mode().cli_value().into(),
                "--model".into(),
                "{model}".into(),
                // The prompt is a stdin JSON user message. `--input-format
                // stream-json` waits for that line and ignores a leftover argv
                // prompt, which left the child idle until the turn timed out.
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

        // Everything the turn wants to show the user goes through one channel.
        // The permission responder needs to render an approval card *and* the
        // stream needs to render text, and both cannot hold `on_event` at once —
        // so neither does. The loop below is the only thing that touches it.
        let (tx, mut rx) = unbounded_channel::<TurnEvent>();
        let mut responder = ClaudePermissions {
            host: req.interaction.clone(),
            root: self.root.clone(),
            cards: tx.clone(),
        };
        let stream_tx = tx.clone();
        let mut on_external_event = move |event: ExternalAgentEvent| {
            let turn_event = match event {
                ExternalAgentEvent::TextDelta(text) => Some(TurnEvent::Text(text)),
                ExternalAgentEvent::Thinking(text) => Some(TurnEvent::Thinking(text)),
                ExternalAgentEvent::ToolCall { id, title, status } => {
                    Some(TurnEvent::Activity { id, title, status })
                }
                _ => None,
            };
            if let Some(turn_event) = turn_event {
                let _ = stream_tx.send(turn_event);
            }
        };
        // Dropped so the channel closes when the runner and responder are done.
        drop(tx);

        let run = {
            let runner = run_headless_command_streaming(
                &self.root,
                &config,
                &prompt,
                req.cancel.as_ref(),
                &mut on_external_event,
                Some(&mut responder),
            );
            tokio::pin!(runner);
            loop {
                tokio::select! {
                    biased;
                    Some(turn_event) = rx.recv() => {
                        emit(turn_event, on_event, &mut streamed_text);
                    }
                    result = &mut runner => {
                        // Drain whatever the final lines queued before settling.
                        while let Ok(turn_event) = rx.try_recv() {
                            emit(turn_event, on_event, &mut streamed_text);
                        }
                        break result?;
                    }
                }
            }
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

/// Anything the turn wants rendered, funnelled through one channel so that
/// exactly one place borrows the event sink.
enum TurnEvent {
    Text(String),
    Thinking(String),
    Activity {
        id: String,
        title: String,
        status: String,
    },
    Approval {
        approval_id: String,
        tool_name: String,
        risk: ToolRisk,
        path: String,
        summary: String,
        diff: String,
    },
}

fn emit(
    event: TurnEvent,
    on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    streamed_text: &mut bool,
) {
    match event {
        TurnEvent::Text(text) => {
            *streamed_text = true;
            on_event(StreamEvent::Text(&text));
        }
        TurnEvent::Thinking(text) => on_event(StreamEvent::Thinking(&text)),
        TurnEvent::Activity { id, title, status } => on_event(StreamEvent::ProviderActivity {
            id: &id,
            title: &title,
            status: &status,
        }),
        TurnEvent::Approval {
            approval_id,
            tool_name,
            risk,
            path,
            summary,
            diff,
        } => on_event(StreamEvent::ApprovalNeeded {
            approval_id: approval_id.clone(),
            tool_name,
            tool_call_id: approval_id,
            risk,
            path,
            summary,
            diff,
        }),
    }
}

/// Turns the CLI's permission requests into zest approval cards.
struct ClaudePermissions {
    host: Option<std::sync::Arc<dyn super::ProviderInteractionHost>>,
    root: PathBuf,
    cards: UnboundedSender<TurnEvent>,
}

#[async_trait]
impl ControlResponder for ClaudePermissions {
    fn prelude(&self, prompt: &str) -> Vec<Value> {
        vec![stream_json_user_message(prompt)]
    }

    async fn respond(&mut self, message: &Value) -> Option<Value> {
        if let Some(request_id) = initialize_request_id(message) {
            return Some(initialize_response(request_id));
        }
        let request = ToolPermissionRequest::parse(message)?;
        let surface = surface_for(&request.tool_name);
        let approval_id = new_id("claude-approval");
        let summary = summarize(&request);
        let (path, diff) = match surface {
            Surface::FileChange => render_diff(&self.root, &request),
            Surface::Command(_) => (String::new(), String::new()),
        };
        let risk = match surface {
            Surface::FileChange => ToolRisk::Write,
            Surface::Command(risk) => risk,
        };

        // Render the card first, then await the answer. The other order shows
        // the user a decision that has already been made.
        let _ = self.cards.send(TurnEvent::Approval {
            approval_id: approval_id.clone(),
            tool_name: request.tool_name.clone(),
            risk,
            path: path.clone(),
            summary: summary.clone(),
            diff: diff.clone(),
        });

        let allowed = decide(
            self.host.as_ref(),
            &approval_id,
            surface,
            path,
            summary,
            diff,
        )
        .await;
        Some(control_response(
            &request.request_id,
            allowed,
            "the user did not approve this tool call",
        ))
    }
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
        .as_ref()
        .map(SystemPrompt::text)
        .filter(|value| !value.trim().is_empty())
    {
        prompt.push_str("# Zest operating context\n\n");
        prompt.push_str(&system);
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

        let interactive = provider.config_for("sonnet").args.join(" ");
        assert!(interactive.contains("--include-partial-messages"));
        // The flag that routes permission prompts to us. Without it the CLI
        // decides locally and denies whatever it cannot auto-approve.
        assert!(
            interactive.contains("--permission-prompt-tool stdio"),
            "{interactive}"
        );
        assert!(
            interactive.contains("--input-format stream-json"),
            "{interactive}"
        );
        assert!(
            !interactive.contains("{prompt}"),
            "stream-json input carries the prompt on stdin, not argv: {interactive}"
        );
        // Configured `accept_edits` auto-approves before the callback is
        // consulted, so an interactive turn must not run in it.
        assert!(
            !interactive.contains("acceptEdits"),
            "accept_edits would silently bypass the approval card: {interactive}"
        );

        // Plan mode is the one configured value that survives, because it does
        // not auto-approve.
        assert_eq!(
            provider.effective_permission_mode(),
            ClaudeCodePermissionMode::Default
        );
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
            allow_tool_use: true,
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
