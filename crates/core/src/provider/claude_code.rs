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
    control_response, decide, grant_target, initialize_request_id, initialize_response,
    prepare_approval, preview_permission, remember_session_grant, render_diff, risk_for,
    stream_json_user_message, summarize, surface_for, Surface, ToolPermissionRequest,
};
use super::claude_stream::ClaudeNormalizer;
use super::stream_contract::{RunOutcome, StreamNormalizer};
use super::turn_spec::{compose, AgentProfile, CompositionCapabilities, ToolScope, TurnSpec};
use super::{
    catalogue, Completion, EffortPolicy, ModelSpec, Provider, ProviderSessionRef, StreamEvent,
    SystemPrompt, TurnRequest,
};
use crate::anthropic::types::Usage;
use crate::auth::{detect_claude_code, AuthStatus};
use crate::config::{
    ClaudeCodePermissionMode, ExternalAgentConfig, ExternalAgentMode, ExternalWorkspace,
    DEFAULT_CLAUDE_CODE_MODEL,
};
use crate::error::{HarnessError, Result};
use crate::thread::new_id;
use crate::tools::approval::{ApprovalDecision, PolicyOutcome, ToolRisk};
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
pub(crate) const BUILTIN_MODELS: &[&str] = &["sonnet", "opus", "haiku", "fable"];

/// The model family the CLI does not accept an `--effort` for.
const NO_EFFORT_FAMILY: &str = "haiku";

/// What Claude Code may reach while it is standing in as Zest's parent agent.
///
/// Passed on `--tools`, which narrows the built-in set. This is a scope
/// boundary, not a permission decision: `--permission-mode` still decides who
/// approves what is inside it. Deliberately *not* `--allowedTools`, which means
/// "run without asking" and would route straight around the approval card.
///
/// Everything left out acts outside the turn or outside Zest's window:
/// `CronCreate` / `CronDelete` / `CronList` / `ScheduleWakeup` schedule work
/// that outlives the chat, `RemoteTrigger` starts cloud agents, `SendMessage`
/// and `PushNotification` talk to things that are not this conversation,
/// `DesignSync` and `ReportFindings` render in Claude Code's own host UI, and
/// `Workflow` fans out to dozens of agents Zest has no way to show. The two
/// worktree tools are excluded for a different reason: Zest already owns
/// worktree isolation through `isolated_workspace`, and a second one opened
/// behind its back is a checkout nothing is tracking.
///
/// `Task` *is* included. A Claude Code subagent stays inside the same session,
/// the same tool scope, and the same approval callback, so it is ordinary work
/// rather than an escape from any of them. Note that the CLI calls the same
/// tool two things: `--tools` and `system:init` name it `Task`, and the
/// `tool_use` block in the stream names it `Agent`. Both have to be spelled
/// correctly in the place that uses them.
pub(crate) const ZEST_TOOL_SCOPE: &[&str] = &[
    "Read",
    "Write",
    "Edit",
    "NotebookEdit",
    "Glob",
    "Grep",
    "Bash",
    "PowerShell",
    "WebSearch",
    "WebFetch",
    "Task",
    "TaskCreate",
    "TaskGet",
    "TaskList",
    "TaskOutput",
    "TaskStop",
    "TaskUpdate",
    "Skill",
    "ToolSearch",
    "Monitor",
];

pub struct ClaudeCodeProvider {
    id: String,
    root: PathBuf,
    command: String,
    default_model: String,
    models: Vec<ModelSpec>,
    allow_mcp: bool,
    permission_mode: ClaudeCodePermissionMode,
    disallowed_tools: Vec<String>,
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
        disallowed_tools: Vec<String>,
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
            models: effort_catalogue(&default_model, &models),
            allow_mcp,
            permission_mode,
            disallowed_tools,
            timeout_secs,
            auth: detect_claude_code(),
        })
    }

    /// The mode the CLI actually runs in.
    ///
    /// This used to collapse every configured mode except `Plan` down to
    /// `default`, because `acceptEdits` and `bypassPermissions` both approve
    /// *before* the permission callback is consulted and would have silently
    /// defeated the approval card. The coercion made the setting a lie: a user
    /// who asked for `accept_edits` got manual approval anyway.
    ///
    /// It is no longer needed. CLI 2.1.220 ships `auto`, a classifier that
    /// approves ordinary work and still routes everything it is unsure about to
    /// the callback, so the card survives without overriding the user. Each
    /// configured mode now means what it says, and the legacy `default` value
    /// maps onto `auto` because that is the mode it was standing in for.
    fn effective_permission_mode(&self) -> ClaudeCodePermissionMode {
        match self.permission_mode {
            ClaudeCodePermissionMode::Default => ClaudeCodePermissionMode::Auto,
            mode => mode,
        }
    }

    /// The tools Claude Code may reach, plus whatever the user vetoed.
    fn tool_scope(&self) -> ToolScope {
        ToolScope::new(ZEST_TOOL_SCOPE.iter().copied()).with_deny(self.disallowed_tools.clone())
    }

    /// Claude Code keeps its own conversation, so instructions and transcript
    /// travel once and `--resume` carries them afterwards.
    fn capabilities() -> CompositionCapabilities {
        CompositionCapabilities::SESSION_OWNING
    }

    /// The session to continue, when the stored one still describes this turn.
    ///
    /// A session holds a model as well as a history: resuming one under a
    /// different model would silently answer from a conversation the new model
    /// never had. The check is made here, before the process starts, so there
    /// is no failed `--resume` to recover from.
    fn resumable_session<'a>(&self, req: &'a TurnRequest) -> Option<&'a str> {
        match req.provider_session.as_ref()? {
            ProviderSessionRef::ClaudeCode { session_id, model } if model == &req.model => {
                Some(session_id.as_str())
            }
            _ => None,
        }
    }

    fn args(&self, req: &TurnRequest, scope: &ToolScope, resume: Option<&str>) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "--print".into(),
            "--verbose".into(),
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
        ];

        if !scope.allow.is_empty() {
            args.push("--tools".into());
            args.push(scope.allow.join(","));
        }
        if !scope.deny.is_empty() {
            args.push("--disallowedTools".into());
            args.push(scope.deny.join(","));
        }

        // `Task` is in scope, so subagents run. Without this their text never
        // reaches the stream and the tool row sits silent for however long the
        // subagent takes.
        if scope.allow.iter().any(|tool| tool == "Task") {
            args.push("--forward-subagent-text".into());
        }

        // The CLI rejects an effort for the small model rather than ignoring it.
        if let Some(effort) = req.effort.as_deref().filter(|effort| !effort.is_empty()) {
            if !req.model.contains(NO_EFFORT_FAMILY) {
                args.push("--effort".into());
                args.push(effort.to_string());
            }
        }

        if let Some(session_id) = resume {
            args.push("--resume".into());
            args.push(session_id.to_string());
        }

        args
    }

    fn config_for(&self, model: &str, args: Vec<String>) -> ExternalAgentConfig {
        ExternalAgentConfig {
            mode: ExternalAgentMode::Headless,
            command: self.command.clone(),
            args,
            allow_mcp: self.allow_mcp,
            model: Some(model.to_string()),
            workspace: ExternalWorkspace::Current,
            timeout_secs: self.timeout_secs,
        }
    }
}

/// Claude Code's catalogue, with efforts where the CLI accepts them.
///
/// The provider used to declare [`EffortPolicy::Unsupported`], which was true
/// when it was written and is not now: CLI 2.1.220 takes `--effort` with the
/// same five levels Zest already calls standard. Haiku is the exception and
/// keeps an empty list, so the picker cannot offer a control that model rejects.
pub(crate) fn effort_catalogue(default_model: &str, models: &[String]) -> Vec<ModelSpec> {
    let mut catalogue = catalogue(
        default_model,
        models,
        BUILTIN_MODELS,
        EffortPolicy::Standard(&[]),
    );
    for model in &mut catalogue {
        if model.id.contains(NO_EFFORT_FAMILY) {
            model.efforts.clear();
        }
    }
    catalogue
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

        let resume = self.resumable_session(req);
        let profile = AgentProfile::parent(self.tool_scope());
        let scope = &profile.tools;
        let system = req.system.as_ref().map(SystemPrompt::text);
        let composed = compose(
            &TurnSpec {
                profile: &profile,
                system: system.as_deref(),
                messages: &req.messages,
                model: &req.model,
                effort: req.effort.as_deref(),
                resumed: resume.is_some(),
                policy_override: None,
            },
            &Self::capabilities(),
        );
        if diagnostics_enabled() {
            for entry in &composed.trace {
                eprintln!(
                    "[claude_code] composition {}={} ({})",
                    entry.step, entry.value, entry.reason
                );
            }
        }

        let prompt = composed.prompt;
        let config = self.config_for(&req.model, self.args(req, scope, resume));
        let mut normalizer =
            ClaudeNormalizer::new(self.root.clone()).requesting_tools(&scope.allow);
        if let Some(session_id) = resume {
            normalizer = normalizer.expecting_session(session_id);
        }
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

        let outcome = {
            let runner = run_headless_command_streaming(
                &self.root,
                &config,
                &prompt,
                req.cancel.as_ref(),
                &mut on_external_event,
                Some(&mut responder),
                Some(&mut normalizer),
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
                        break result;
                    }
                }
            }
        };

        // The report is the only place a schema change shows up. Take it on
        // every ending, because a stream that lost a record it needed is a more
        // likely explanation for a failure than for a success.
        report_stream_health(&normalizer, &outcome);
        let run = outcome?;

        if req
            .cancel
            .as_ref()
            .is_some_and(|cancel| cancel.is_cancelled())
        {
            return Err(HarnessError::Cancelled);
        }

        // Resuming into a conversation the CLI substituted would answer from a
        // history nobody asked for, so the session is not carried forward.
        let session = match normalizer.session_mismatch() {
            Some(_) => None,
            None => normalizer
                .session_id()
                .map(|session_id| ProviderSessionRef::ClaudeCode {
                    session_id: session_id.to_string(),
                    model: req.model.clone(),
                }),
        };

        let answer = run.text();
        if answer.trim().is_empty() {
            // Tag these so the desktop shows Claude's words. `Other` is
            // treated as internal and becomes "Try again."
            if let Some(error) = normalizer.result_error() {
                return Err(HarnessError::from_provider_stream("claude_code", error));
            }
            if let Some(error) = run.errors().first() {
                return Err(HarnessError::from_provider_stream("claude_code", *error));
            }
            // "No answer" and "stopped before answering" are different failures
            // and want different next steps, so they should not read the same.
            // The stream always ends in a `result` record when the turn really
            // finished; reaching here without one means the CLI stopped early.
            if !normalizer.has_finish() {
                return Err(HarnessError::from_provider_stream(
                    "claude_code",
                    "Claude Code stopped before finishing this turn.",
                ));
            }
            return Err(HarnessError::from_provider_stream(
                "claude_code",
                "Claude Code returned no answer.",
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
            provider_session: session,
        })
    }
}

/// Opt-in diagnostics, matching the ACP timing switch: no prompts, no output
/// text, no workspace paths.
fn diagnostics_enabled() -> bool {
    std::env::var_os("ZEST_CLAUDE_CODE_DIAGNOSTICS").is_some_and(|value| !value.is_empty())
}

/// Report what the stream looked like, when it did not look like the contract.
///
/// Always on rather than behind the diagnostics switch: silent schema drift is
/// exactly the failure this cannot afford to hide, and the message names record
/// kinds only.
fn report_stream_health(
    normalizer: &ClaudeNormalizer,
    outcome: &Result<crate::tools::external_agent::ExternalAgentRun>,
) {
    let run_outcome = match outcome {
        Ok(_) => RunOutcome::Success,
        Err(HarnessError::Cancelled) => RunOutcome::Abort,
        Err(_) => RunOutcome::Error,
    };
    if let Some(message) = normalizer.report(run_outcome).describe() {
        eprintln!("[claude_code] {message}");
    }
    if !normalizer.dropped_tools().is_empty() {
        eprintln!(
            "[claude_code] the CLI did not recognise these tools: {}",
            normalizer.dropped_tools().join(", ")
        );
    }
    if let Some(session_id) = normalizer.session_mismatch() {
        eprintln!(
            "[claude_code] the CLI answered from session {session_id}, which is not the one \
             requested; the session was not carried forward"
        );
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
    /// Close the Zest tool row after a decision. Without this the card stays
    /// `awaiting_approval`, sits at the head of the queue, and the next Allow
    /// hits a waiter that is already gone ("approval expired").
    ApprovalSettled {
        approval_id: String,
        tool_name: String,
        summary: String,
        allowed: bool,
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
        TurnEvent::ApprovalSettled {
            approval_id,
            tool_name,
            summary,
            allowed,
        } => on_event(StreamEvent::ToolCallResult {
            name: &tool_name,
            id: &approval_id,
            summary: &summary,
            is_error: !allowed,
            path: None,
            diff: None,
            metadata: None,
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
        let risk = risk_for(&request);
        let target = grant_target(&request, &path, &summary);
        let policy = self.host.as_ref().and_then(|host| host.approval_policy());

        // The CLI asks about every tool, including ordinary reads. Native Zest
        // tools do not. Consult the session policy before drawing a card so
        // Auto and "Allow for session" are not no-ops that re-ask on the next
        // slightly different path.
        match preview_permission(policy.as_ref(), &request.tool_name, &target, risk) {
            PolicyOutcome::Allow => {
                return Some(control_response(&request.request_id, true, ""));
            }
            PolicyOutcome::Block(reason) => {
                return Some(control_response(&request.request_id, false, &reason));
            }
            PolicyOutcome::Ask => {}
        }

        prepare_approval(self.host.as_ref(), &approval_id, surface).await;
        let _ = self.cards.send(TurnEvent::Approval {
            approval_id: approval_id.clone(),
            tool_name: request.tool_name.clone(),
            risk,
            path: path.clone(),
            summary: summary.clone(),
            diff: diff.clone(),
        });

        let decision = decide(
            self.host.as_ref(),
            &approval_id,
            surface,
            path,
            summary.clone(),
            diff,
        )
        .await;
        if decision == ApprovalDecision::AllowSession {
            if let Some(policy) = &policy {
                if let Ok(mut guard) = policy.lock() {
                    remember_session_grant(&mut guard, surface, &request.tool_name, &target);
                }
            }
        }
        let allowed = matches!(
            decision,
            ApprovalDecision::AllowOnce | ApprovalDecision::AllowSession
        );
        let _ = self.cards.send(TurnEvent::ApprovalSettled {
            approval_id,
            tool_name: request.tool_name.clone(),
            summary,
            allowed,
        });
        Some(control_response(
            &request.request_id,
            allowed,
            "the user did not approve this tool call",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::types::Message;

    fn provider(permission_mode: ClaudeCodePermissionMode) -> ClaudeCodeProvider {
        ClaudeCodeProvider::new(
            "claude",
            ".",
            "claude",
            None,
            Vec::new(),
            false,
            permission_mode,
            Vec::new(),
            900,
        )
        .unwrap()
    }

    fn request(model: &str) -> TurnRequest {
        TurnRequest {
            model: model.into(),
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
        }
    }

    fn args(provider: &ClaudeCodeProvider, req: &TurnRequest) -> String {
        let scope = provider.tool_scope();
        let resume = provider.resumable_session(req);
        provider.args(req, &scope, resume).join(" ")
    }

    fn prompt(provider: &ClaudeCodeProvider, req: &TurnRequest, resumed: bool) -> String {
        let system = req.system.as_ref().map(SystemPrompt::text);
        let profile = AgentProfile::parent(provider.tool_scope());
        compose(
            &TurnSpec {
                profile: &profile,
                system: system.as_deref(),
                messages: &req.messages,
                model: &req.model,
                effort: req.effort.as_deref(),
                resumed,
                policy_override: None,
            },
            &ClaudeCodeProvider::capabilities(),
        )
        .prompt
    }

    #[test]
    fn default_model_and_aliases_are_available() {
        let provider = provider(ClaudeCodePermissionMode::AcceptEdits);

        assert_eq!(provider.default_model(), "sonnet");
        assert!(provider.models().iter().any(|model| model.id == "opus"));
        assert!(provider.models().iter().any(|model| model.id == "fable"));
        assert!(provider.owns_agent_loop());
    }

    #[test]
    fn config_requests_live_claude_stream_events() {
        let provider = provider(ClaudeCodePermissionMode::AcceptEdits);
        let interactive = args(&provider, &request("sonnet"));

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
    }

    /// The provider used to collapse every mode except `plan` onto `default`,
    /// because `acceptEdits` approved before the callback was consulted. That
    /// override made the setting a lie: asking for `accept_edits` still got
    /// manual approval. `auto` refers what it is unsure about back to the
    /// callback, so each mode can now mean what it says.
    #[test]
    fn a_configured_permission_mode_reaches_the_cli() {
        for (configured, expected) in [
            (ClaudeCodePermissionMode::Auto, "auto"),
            (ClaudeCodePermissionMode::Manual, "manual"),
            (ClaudeCodePermissionMode::AcceptEdits, "acceptEdits"),
            (ClaudeCodePermissionMode::Plan, "plan"),
            (
                ClaudeCodePermissionMode::BypassPermissions,
                "bypassPermissions",
            ),
        ] {
            let provider = provider(configured);
            let interactive = args(&provider, &request("sonnet"));
            assert!(
                interactive.contains(&format!("--permission-mode {expected}")),
                "{configured:?}: {interactive}"
            );
        }
    }

    /// `default` is not among the CLI's documented choices. It is still taken
    /// today, but it stands for what `auto` now names, so nothing passes it on.
    #[test]
    fn the_legacy_default_mode_resolves_to_auto() {
        let provider = provider(ClaudeCodePermissionMode::Default);
        assert_eq!(
            provider.effective_permission_mode(),
            ClaudeCodePermissionMode::Auto
        );
        assert!(args(&provider, &request("sonnet")).contains("--permission-mode auto"));
    }

    /// The scope boundary. `--allowedTools` means "run without asking" and must
    /// never appear: it would route straight around the approval card.
    #[test]
    fn the_tool_scope_narrows_the_cli_without_auto_approving() {
        let provider = provider(ClaudeCodePermissionMode::Auto);
        let interactive = args(&provider, &request("sonnet"));

        assert!(interactive.contains("--tools "), "{interactive}");
        assert!(
            !interactive.contains("--allowedTools"),
            "an allow-list bypasses the approval card: {interactive}"
        );
        for inside in ["Read", "Write", "Edit", "Bash", "PowerShell", "Task"] {
            assert!(
                ZEST_TOOL_SCOPE.contains(&inside),
                "{inside} should be in scope"
            );
        }
        for outside in [
            "CronCreate",
            "ScheduleWakeup",
            "RemoteTrigger",
            "PushNotification",
            "SendMessage",
            "DesignSync",
            "ReportFindings",
            "EnterWorktree",
            "ExitWorktree",
            "Workflow",
        ] {
            assert!(
                !ZEST_TOOL_SCOPE.contains(&outside),
                "{outside} acts outside the turn and should be out of scope"
            );
        }
    }

    /// `--tools` drops names it does not recognise in silence, so a wrong entry
    /// narrows the agent with nothing to show for it. Anygent's list carries
    /// `LS` and `TodoWrite`, which CLI 2.1.220 does not ship, and `Agent`, which
    /// is the name of the *call* in the stream rather than a name `--tools`
    /// accepts. The scope has to use `Task`, which is what `--tools` takes.
    #[test]
    fn the_scope_uses_the_names_the_tools_flag_accepts() {
        for wrong in ["LS", "TodoWrite", "Agent"] {
            assert!(
                !ZEST_TOOL_SCOPE.contains(&wrong),
                "`--tools` does not accept {wrong} and would drop it in silence"
            );
        }
        assert!(ZEST_TOOL_SCOPE.contains(&"Task"));
    }

    #[test]
    fn a_configured_deny_list_is_layered_on_the_scope() {
        let provider = ClaudeCodeProvider::new(
            "claude",
            ".",
            "claude",
            None,
            Vec::new(),
            false,
            ClaudeCodePermissionMode::Auto,
            vec!["Bash".to_string()],
            900,
        )
        .unwrap();

        let interactive = args(&provider, &request("sonnet"));
        assert!(
            interactive.contains("--disallowedTools Bash"),
            "{interactive}"
        );
        assert!(interactive.contains("--tools "), "{interactive}");
    }

    /// `Task` is in scope, so subagents run. Without this flag their text never
    /// reaches the stream and the tool row sits silent for as long as they take.
    #[test]
    fn subagent_output_is_forwarded_because_task_is_in_scope() {
        let provider = provider(ClaudeCodePermissionMode::Auto);
        assert!(args(&provider, &request("sonnet")).contains("--forward-subagent-text"));
    }

    #[test]
    fn effort_reaches_the_cli_except_on_the_model_that_rejects_it() {
        let provider = provider(ClaudeCodePermissionMode::Auto);

        let mut sonnet = request("sonnet");
        sonnet.effort = Some("xhigh".into());
        assert!(args(&provider, &sonnet).contains("--effort xhigh"));

        let mut haiku = request("haiku");
        haiku.effort = Some("xhigh".into());
        assert!(!args(&provider, &haiku).contains("--effort"));
    }

    /// The picker must not offer a control the model rejects.
    #[test]
    fn the_catalogue_offers_effort_only_where_the_cli_takes_it() {
        let provider = provider(ClaudeCodePermissionMode::Auto);
        let efforts = |id: &str| {
            provider
                .models()
                .into_iter()
                .find(|model| model.id == id)
                .map(|model| model.efforts)
                .expect(id)
        };

        assert!(efforts("sonnet").contains(&"xhigh".to_string()));
        assert!(efforts("opus").contains(&"low".to_string()));
        assert!(efforts("haiku").is_empty());
    }

    #[test]
    fn a_fresh_turn_carries_the_whole_conversation() {
        let provider = provider(ClaudeCodePermissionMode::Auto);
        let req = request("sonnet");

        assert!(provider.resumable_session(&req).is_none());
        assert!(!args(&provider, &req).contains("--resume"));

        let opening = prompt(&provider, &req, false);
        assert!(opening.contains("Follow the project rules."));
        assert!(opening.contains("Inspect the loader."));
        assert!(opening.contains("Now implement the fix."));
    }

    #[test]
    fn a_stored_session_is_resumed_instead_of_replayed() {
        let provider = provider(ClaudeCodePermissionMode::Auto);
        let mut req = request("sonnet");
        req.provider_session = Some(ProviderSessionRef::ClaudeCode {
            session_id: "session-a".into(),
            model: "sonnet".into(),
        });

        assert_eq!(provider.resumable_session(&req), Some("session-a"));
        assert!(args(&provider, &req).contains("--resume session-a"));
        // The whole point: the operating context and the transcript were
        // delivered when the session opened and are not sent again.
        assert_eq!(prompt(&provider, &req, true), "Now implement the fix.");
    }

    /// A session holds the model that produced it. Resuming one under another
    /// model would answer from a conversation that model never had, and the CLI
    /// would do it without complaining.
    #[test]
    fn a_session_from_another_model_is_not_resumed() {
        let provider = provider(ClaudeCodePermissionMode::Auto);
        let mut req = request("opus");
        req.provider_session = Some(ProviderSessionRef::ClaudeCode {
            session_id: "session-a".into(),
            model: "sonnet".into(),
        });

        assert_eq!(provider.resumable_session(&req), None);
        assert!(!args(&provider, &req).contains("--resume"));
    }

    #[test]
    fn another_providers_session_is_never_resumed_here() {
        let provider = provider(ClaudeCodePermissionMode::Auto);
        let mut req = request("sonnet");
        req.provider_session = Some(ProviderSessionRef::CursorAcp {
            session_id: "session-a".into(),
        });

        assert_eq!(provider.resumable_session(&req), None);
    }

    /// The old prompt told Claude not to delegate. Subagents are now inside the
    /// tool scope, so that instruction would forbid work the scope allows.
    #[test]
    fn the_prompt_no_longer_forbids_subagents() {
        let provider = provider(ClaudeCodePermissionMode::Auto);
        let opening = prompt(&provider, &request("sonnet"), false);

        assert!(!opening.contains("Do not delegate"), "{opening}");
    }

    /// Explicitly opt-in: consumes the signed-in Claude Code subscription.
    ///
    /// Two turns, so the thing this pass is actually about gets exercised: the
    /// first opens a session and the second has to resume it and send only the
    /// new message. Everything below the CLI boundary is covered by the unit
    /// tests; this is the part where the CLI has to agree.
    #[tokio::test]
    #[ignore = "live Claude Code turn; requires `claude auth login`"]
    async fn live_claude_code_session_survives_a_second_turn() {
        let root = std::env::var("ZEST_LIVE_ROOT").unwrap_or_else(|_| ".".into());
        let provider = ClaudeCodeProvider::new(
            "claude",
            root,
            "claude",
            Some("haiku".into()),
            Vec::new(),
            false,
            ClaudeCodePermissionMode::Auto,
            Vec::new(),
            120,
        )
        .unwrap();

        let mut messages = vec![Message::user_text(
            "Reply with exactly the word ALPHA and nothing else.",
        )];
        let mut session = None;

        for (turn, expected) in [(1, "ALPHA"), (2, "BETA")] {
            let req = TurnRequest {
                model: "haiku".into(),
                system: Some("You are being driven by an automated test.".into()),
                messages: messages.clone(),
                tools: Vec::new(),
                allow_tool_use: false,
                max_tokens: 256,
                effort: None,
                thinking: false,
                provider_session: session.clone(),
                interaction: None,
                cancel: None,
            };

            let mut text = String::new();
            let completion = tokio::time::timeout(
                std::time::Duration::from_secs(180),
                provider.stream_turn(&req, &mut |event| {
                    if let StreamEvent::Text(chunk) = event {
                        text.push_str(chunk);
                    }
                }),
            )
            .await
            .expect("turn exceeded 180 seconds")
            .expect("live Claude Code turn failed");

            let answer = completion.content[0]["text"].as_str().unwrap_or_default();
            println!("turn {turn}: session={session:?} answer={answer:?}");
            assert!(
                answer.contains(expected),
                "turn {turn} should say {expected}: {answer}"
            );

            let next = completion.provider_session.clone();
            assert!(
                matches!(next, Some(ProviderSessionRef::ClaudeCode { .. })),
                "turn {turn} must carry a session forward: {next:?}"
            );
            if turn == 1 {
                messages.push(Message::assistant(vec![
                    json!({"type": "text", "text": answer}),
                ]));
                messages.push(Message::user_text(
                    "Now reply with exactly the word BETA and nothing else.",
                ));
            } else {
                assert_eq!(
                    session, next,
                    "the second turn must resume the first turn's session"
                );
            }
            session = next;
        }
    }
}
