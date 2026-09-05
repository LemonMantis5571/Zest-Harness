//! Cursor CLI as a parent provider, over the Agent Client Protocol.
//!
//! `cursor-agent acp` speaks JSON-RPC 2.0 over stdio, and Cursor owns the
//! authenticated subscription session (`cursor-agent login`) along with its own
//! model and tool loop. Zest supplies the workspace, the parent conversation,
//! and the approval host, and never reads Cursor's credentials.
//!
//! This is the third provider of that species, after Claude Code and the Codex
//! app-server, and it is closest to the latter: one JSON-RPC process per turn,
//! notifications rendered as provider activity, server-initiated requests
//! answered through [`ProviderInteractionHost`].
//!
//! # What Cursor does *not* gate
//!
//! Measured against `cursor-agent acp` (see `scripts/acp-probe.mjs`): Cursor
//! sends `session/request_permission` only for shell commands outside its own
//! allowlist. **File edits are never announced for approval.** A refused turn
//! still edited the file on disk. It also ignores the client's `fs`
//! capabilities — advertising `readTextFile`/`writeTextFile` does not make it
//! route reads and writes back through us; it uses its own tools with absolute
//! paths. So the capabilities below are declared `false`, which is the honest
//! statement of who does the work.
//!
//! The consequence is deliberate and must not be papered over: on this provider
//! the command approval card is real and the file-change one cannot exist. The
//! `plan` and `ask` session modes are the only mechanism that stops edits, which
//! is why [`CursorMode`] is part of the configuration rather than a detail.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::cursor_models;
use super::session::JsonlProcess;
use super::{
    context_window_for_model, Completion, ModelSpec, Provider, ProviderCommandRequest,
    ProviderInteractionHost, ProviderQuestionRequest, StreamEvent, SystemPrompt, TurnRequest,
};
use crate::anthropic::types::Usage;
use crate::auth::{detect_cursor_cli, AuthStatus};
use crate::config::CursorMode;
use crate::error::{HarnessError, Result};
use crate::thread::new_id;
use crate::tools::approval::ToolRisk;
use crate::tools::external_agent::{
    prepare_external_command, resolve_program, scrub_secret_environment,
    scrub_zest_secret_environment,
};

const CLIENT_NAME: &str = "zest";
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const PROTOCOL_VERSION: u64 = 1;

/// The model Cursor selects when nothing is pinned. `default[]` is its own id
/// for Auto, which is what `session/new` reports on a fresh session.
pub const DEFAULT_CURSOR_MODEL: &str = "composer-2.5";

/// What the catalogue offers, shared with the driver so the picker and the live
/// provider cannot disagree about which models exist.
///
/// An explicit `models` allow-list in `zest.toml` wins; otherwise the account's
/// own catalogue is discovered from the CLI and cached. A hand-written list is
/// wrong for everyone but its author — see [`cursor_models`].
pub(crate) fn model_catalogue(
    command: &str,
    default_model: &str,
    configured: &[String],
) -> Vec<ModelSpec> {
    let mut models = if configured.is_empty() {
        cursor_models::catalogue(command)
    } else {
        configured
            .iter()
            .map(|id| ModelSpec {
                context_window: context_window_for_model(id),
                id: id.clone(),
                // Cursor bakes effort into the flat id, so a hand-pinned entry
                // is taken exactly as written rather than being offered a
                // ladder that would be appended to a name that already has one.
                efforts: Vec::new(),
                supports_tools: true,
                supports_vision: true,
            })
            .collect()
    };
    // A configured default the catalogue rejects is a startup failure, so it is
    // always present and always first.
    if !models.iter().any(|model| model.id == default_model) {
        models.insert(
            0,
            ModelSpec {
                context_window: context_window_for_model(default_model),
                id: default_model.to_string(),
                efforts: Vec::new(),
                supports_tools: true,
                supports_vision: true,
            },
        );
    }
    models
}

pub struct CursorAcpProvider {
    id: String,
    root: PathBuf,
    command: String,
    default_model: String,
    models: Vec<ModelSpec>,
    allow_mcp: bool,
    mode: CursorMode,
    timeout_secs: u64,
    auth: AuthStatus,
}

impl CursorAcpProvider {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        root: impl Into<PathBuf>,
        command: impl Into<String>,
        model: Option<String>,
        models: Vec<String>,
        allow_mcp: bool,
        mode: CursorMode,
        timeout_secs: u64,
    ) -> Result<Self> {
        let command = command.into();
        if command.trim().is_empty() {
            return Err(HarnessError::Other("Cursor command cannot be empty".into()));
        }
        if timeout_secs == 0 || timeout_secs > 3_600 {
            return Err(HarnessError::Other(
                "Cursor timeout_secs must be between 1 and 3600".into(),
            ));
        }
        let default_model = model
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_CURSOR_MODEL.to_string());
        Ok(Self {
            id: id.into(),
            root: root.into(),
            models: model_catalogue(&command, &default_model, &models),
            command,
            default_model,
            allow_mcp,
            mode,
            timeout_secs,
            auth: detect_cursor_cli(),
        })
    }

    fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs.max(1))
    }

    async fn spawn(&self, model: &str, effort: Option<&str>) -> Result<JsonlProcess> {
        // Cursor reads its options *before* the subcommand, the way its own
        // docs write `agent --api-key "$KEY" acp`. The model is pinned here
        // because ACP has no per-session model parameter: `session/new`
        // reports the model back, it does not accept one.
        //
        // Effort is folded back into the id: Cursor spells it as a suffix
        // (`cursor-grok-4.6-high`), and the catalogue split it off so Zest's
        // own effort selector could drive it.
        let mut args = Vec::new();
        let wire = cursor_models::wire_model(model.trim(), effort);
        if !wire.is_empty() {
            args.push("--model".to_string());
            args.push(wire);
        }
        args.push("acp".to_string());

        // Resolve before spawning: the Windows installer ships `cursor-agent.cmd`
        // with no `.exe`, which a bare program name cannot reach.
        let mut command = tokio::process::Command::new(resolve_program(&self.command));
        command.args(&args).current_dir(&self.root);
        prepare_external_command(&mut command);
        // Cursor authenticates through its own store, so Zest's provider
        // credentials are always withheld. When it may run MCP servers those
        // children need the user's environment, so only Zest's keys go.
        if self.allow_mcp {
            scrub_zest_secret_environment(&mut command, &[]);
        } else {
            scrub_secret_environment(&mut command, &[]);
        }
        JsonlProcess::spawn_command(command, &self.command).await
    }

    async fn run_turn(
        &self,
        req: &TurnRequest,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    ) -> Result<Completion> {
        let mut process = self.spawn(&req.model, req.effort.as_deref()).await?;
        let mut next_id = 1_u64;
        let mut state = TurnState::default();

        let initialized = rpc_request(
            &mut process,
            &mut next_id,
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "clientInfo": {"name": CLIENT_NAME, "version": CLIENT_VERSION},
                // Declared false because Cursor ignores them anyway: it reads
                // and writes with its own tools. Claiming otherwise would
                // suggest an interception point that does not exist.
                "clientCapabilities": {
                    "fs": {"readTextFile": false, "writeTextFile": false},
                    "terminal": false
                }
            }),
            self,
            req,
            &mut state,
            on_event,
        )
        .await?;

        // A CLI that has never been signed in answers session/new with
        // "Authentication required", which is a sign-in problem rather than a
        // protocol one. Say so with the command that fixes it.
        let needs_login = matches!(self.auth, AuthStatus::NotLoggedIn { .. })
            && initialized.get("authMethods").is_some();

        let session = rpc_request(
            &mut process,
            &mut next_id,
            "session/new",
            json!({"cwd": self.root.display().to_string(), "mcpServers": []}),
            self,
            req,
            &mut state,
            on_event,
        )
        .await
        .map_err(|error| {
            if needs_login {
                HarnessError::Other("Cursor is not signed in. Run `cursor-agent login`.".into())
            } else {
                error
            }
        })?;

        let session_id = session
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| HarnessError::Other("Cursor session/new returned no sessionId".into()))?
            .to_string();
        let served_model = session
            .pointer("/models/currentModelId")
            .and_then(Value::as_str)
            .map(str::to_string);

        // Mode is the only lever that stops edits, so a failure to set it is a
        // failure of the turn's safety contract, not a cosmetic one.
        if self.mode != CursorMode::Agent {
            rpc_request(
                &mut process,
                &mut next_id,
                "session/set_mode",
                json!({"sessionId": session_id, "modeId": self.mode.wire_value()}),
                self,
                req,
                &mut state,
                on_event,
            )
            .await?;
        }

        let result = rpc_request(
            &mut process,
            &mut next_id,
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": prompt_for_turn(req)}]
            }),
            self,
            req,
            &mut state,
            on_event,
        )
        .await?;

        process.close_stdin();
        let stop_reason = result
            .get("stopReason")
            .and_then(Value::as_str)
            .map(str::to_string);

        let text = state.text.trim().to_string();
        if text.is_empty() {
            return Err(HarnessError::Other(
                "Cursor ended the turn without an answer".into(),
            ));
        }
        Ok(Completion {
            content: vec![json!({"type": "text", "text": text})],
            stop_reason,
            usage: Usage::default(),
            // ACP carries no token accounting, and inventing one would make the
            // usage meter lie rather than admit it does not know.
            usage_available: false,
            limits: None,
            served_model,
            provider_session: None,
        })
    }
}

#[async_trait]
impl Provider for CursorAcpProvider {
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
        self.run_turn(req, on_event).await
    }
}

#[derive(Default)]
struct TurnState {
    text: String,
}

/// Send one request and pump everything that arrives until its response does.
///
/// Notifications and server-initiated requests both arrive while a request is
/// outstanding, and a server request left unanswered blocks Cursor rather than
/// failing it, so this loop must answer every one it sees.
#[allow(clippy::too_many_arguments)]
async fn rpc_request(
    process: &mut JsonlProcess,
    next_id: &mut u64,
    method: &str,
    params: Value,
    provider: &CursorAcpProvider,
    req: &TurnRequest,
    state: &mut TurnState,
    on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
) -> Result<Value> {
    let id = *next_id;
    *next_id = next_id.saturating_add(1);
    process
        .send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
        .await?;

    loop {
        let Some(message) = process
            .next(provider.timeout(), req.cancel.as_ref())
            .await?
        else {
            return Err(HarnessError::PrematureEof);
        };

        if message.get("id").and_then(Value::as_u64) == Some(id) && message.get("method").is_none()
        {
            if let Some(error) = message.get("error") {
                return Err(protocol_error(method, error));
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }

        let Some(incoming) = message.get("method").and_then(Value::as_str) else {
            continue;
        };
        match message.get("id") {
            Some(request_id) => {
                let result =
                    server_request_result(incoming, message.get("params"), req, on_event).await;
                process
                    .send(&json!({"jsonrpc": "2.0", "id": request_id, "result": result}))
                    .await?;
            }
            None => absorb_notification(incoming, message.get("params"), state, on_event),
        }
    }
}

/// Render a `session/update`, and ignore the rest.
///
/// Cursor's own `cursor/*` notifications (todos, subagent tasks, generated
/// images) are display sugar for its IDE. Dropping them is deliberate: showing
/// half of a feature Zest cannot complete reads as a bug.
fn absorb_notification(
    method: &str,
    params: Option<&Value>,
    state: &mut TurnState,
    on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
) {
    if method != "session/update" {
        return;
    }
    let Some(update) = params.and_then(|params| params.get("update")) else {
        return;
    };
    match update.get("sessionUpdate").and_then(Value::as_str) {
        Some("agent_message_chunk") => {
            if let Some(text) = content_text(update.get("content")) {
                state.text.push_str(&text);
                on_event(StreamEvent::Text(&text));
            }
        }
        Some("agent_thought_chunk") => {
            if let Some(text) = content_text(update.get("content")) {
                on_event(StreamEvent::Thinking(&text));
            }
        }
        Some("tool_call") | Some("tool_call_update") => {
            // Provider-owned activity: Cursor runs these itself, and the id is
            // what ties a later update to the row already on screen.
            let id = update
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let title = update
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Working");
            let status = update
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("in_progress");
            if !id.is_empty() {
                on_event(StreamEvent::ProviderActivity { id, title, status });
            }
        }
        _ => {}
    }
}

/// Answer a request Cursor made of us. Never guess an approval: with no host to
/// ask, the answer is no.
async fn server_request_result(
    method: &str,
    params: Option<&Value>,
    req: &TurnRequest,
    on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
) -> Value {
    let params = params.cloned().unwrap_or(Value::Null);
    let interaction = req.interaction.clone();
    match method {
        "session/request_permission" => permission_result(&params, interaction, on_event).await,
        "cursor/ask_question" => question_result(&params, interaction).await,
        // Blocking, and approving a plan nobody read is not an approval.
        "cursor/create_plan" => json!({"approved": false}),
        _ => Value::Null,
    }
}

async fn permission_result(
    params: &Value,
    interaction: Option<Arc<dyn ProviderInteractionHost>>,
    on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
) -> Value {
    let tool_call = params.get("toolCall");
    let approval_id = tool_call
        .and_then(|call| call.get("toolCallId"))
        .and_then(Value::as_str)
        // Cursor's ids embed a newline, which is not something to carry into a
        // card id that the desktop matches on.
        .map(|id| id.replace(['\n', '\r'], " "))
        .unwrap_or_else(|| new_id("provider-approval"));
    let summary = tool_call
        .and_then(|call| call.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("Cursor requested permission")
        .to_string();
    let reason = tool_call
        .and_then(|call| call.get("content"))
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| content_text(item.get("content")));

    if let Some(host) = interaction.as_ref() {
        host.prepare_command_approval(&approval_id).await;
    }
    on_event(StreamEvent::ApprovalNeeded {
        approval_id: approval_id.clone(),
        tool_name: "cursor_command".into(),
        tool_call_id: approval_id.clone(),
        risk: ToolRisk::Exec,
        path: String::new(),
        summary: summary.clone(),
        diff: String::new(),
    });

    let approved = match interaction {
        Some(host) => {
            host.approve_command(ProviderCommandRequest {
                approval_id,
                command: summary,
                cwd: None,
                reason,
            })
            .await
        }
        None => false,
    };

    // Only ever allow *once*. `allow-always` is not ours to grant: Cursor
    // persists it into the user's own `~/.cursor/cli-config.json` allowlist,
    // where it silently outlives this turn, this chat, and Zest itself.
    let option_id = pick_option(
        params,
        if approved {
            "allow-once"
        } else {
            "reject-once"
        },
    );
    json!({"outcome": {"outcome": "selected", "optionId": option_id}})
}

async fn question_result(
    params: &Value,
    interaction: Option<Arc<dyn ProviderInteractionHost>>,
) -> Value {
    let choices = params
        .get("options")
        .or_else(|| params.get("choices"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.get("label")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let prompt = params
        .get("question")
        .and_then(Value::as_str)
        .unwrap_or("Cursor asked a question")
        .to_string();
    let id = new_id("provider-question");

    let Some(host) = interaction else {
        return Value::Null;
    };
    host.prepare_question(&id).await;
    let answer = host
        .answer_question(ProviderQuestionRequest {
            id,
            prompt,
            choices,
            multiple: false,
        })
        .await;
    match answer.and_then(|values| values.into_iter().next()) {
        Some(value) => json!({"answer": value}),
        None => Value::Null,
    }
}

/// Choose an offered option by id, falling back to the id we wanted.
///
/// Reading the list rather than assuming it is what keeps a future Cursor
/// release from silently turning a rejection into whatever sits first.
fn pick_option(params: &Value, wanted: &str) -> String {
    params
        .get("options")
        .and_then(Value::as_array)
        .and_then(|options| {
            options
                .iter()
                .filter_map(|option| option.get("optionId").and_then(Value::as_str))
                .find(|id| *id == wanted)
        })
        .unwrap_or(wanted)
        .to_string()
}

/// ACP content blocks are `{type:"text",text}`, sometimes nested one deep.
fn content_text(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    content_text(value.get("content"))
}

/// Cursor keeps its own conversation, so only the newest user message is sent.
/// The system prompt rides in front of it because ACP has no system field.
fn prompt_for_turn(req: &TurnRequest) -> String {
    let latest = req
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| {
            message
                .content
                .iter()
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    match req.system.as_ref().map(SystemPrompt::text) {
        Some(system) if !system.trim().is_empty() => format!("{system}\n\n{latest}"),
        _ => latest,
    }
}

fn protocol_error(method: &str, error: &Value) -> HarnessError {
    let message = error
        .get("data")
        .and_then(|data| data.get("message"))
        .or_else(|| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("unknown error");
    HarnessError::Other(format!("Cursor rejected {method}: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::types::Message;

    /// Always built with an explicit model list. An empty one would send
    /// `model_catalogue` off to discover the real account catalogue, making the
    /// test depend on whoever is signed in on the machine running it.
    fn provider() -> CursorAcpProvider {
        CursorAcpProvider::new(
            "cursor",
            std::env::temp_dir(),
            "cursor-agent",
            None,
            vec!["composer-2.5".into(), "cursor-grok-4.6-high".into()],
            false,
            CursorMode::Agent,
            900,
        )
        .unwrap()
    }

    #[test]
    fn owns_its_own_agent_loop_so_zest_registers_no_tools() {
        assert!(provider().owns_agent_loop());
    }

    #[test]
    fn a_configured_list_is_taken_literally_and_offered_no_effort_ladder() {
        // A hand-pinned id already carries its own effort suffix, so appending
        // one would produce `cursor-grok-4.6-high-high`.
        let models = provider().models();
        let grok = models
            .iter()
            .find(|model| model.id == "cursor-grok-4.6-high")
            .expect("configured id is offered verbatim");
        assert!(grok.efforts.is_empty());

        let pinned = CursorAcpProvider::new(
            "cursor",
            std::env::temp_dir(),
            "cursor-agent",
            Some("gpt-5.6-sol".into()),
            vec!["gpt-5.6-sol".into()],
            false,
            CursorMode::Agent,
            900,
        )
        .unwrap();
        assert_eq!(pinned.default_model(), "gpt-5.6-sol");
        assert_eq!(
            pinned
                .models()
                .into_iter()
                .map(|model| model.id)
                .collect::<Vec<_>>(),
            vec!["gpt-5.6-sol".to_string()]
        );
    }

    #[test]
    fn an_empty_command_or_absurd_timeout_is_refused() {
        assert!(CursorAcpProvider::new(
            "cursor",
            std::env::temp_dir(),
            "  ",
            None,
            Vec::new(),
            false,
            CursorMode::Agent,
            900
        )
        .is_err());
        assert!(CursorAcpProvider::new(
            "cursor",
            std::env::temp_dir(),
            "cursor-agent",
            None,
            Vec::new(),
            false,
            CursorMode::Agent,
            0
        )
        .is_err());
    }

    #[test]
    fn only_the_newest_user_message_is_sent_because_cursor_keeps_the_thread() {
        let req = TurnRequest {
            model: "composer-2.5".into(),
            system: Some(SystemPrompt::new("be brief")),
            messages: vec![
                Message::user_text("first"),
                Message::assistant(vec![json!({"type": "text", "text": "answer"})]),
                Message::user_text("second"),
            ],
            tools: Vec::new(),
            allow_tool_use: false,
            max_tokens: 64,
            effort: None,
            thinking: false,
            provider_session: None,
            interaction: None,
            cancel: None,
        };
        let prompt = prompt_for_turn(&req);
        assert!(prompt.starts_with("be brief"), "{prompt}");
        assert!(prompt.ends_with("second"), "{prompt}");
        assert!(!prompt.contains("first"), "{prompt}");
    }

    #[test]
    fn a_permission_answer_never_upgrades_itself_to_allow_always() {
        // Cursor writes allow-always into the user's global CLI allowlist,
        // where it outlives the turn. Zest must never select it.
        let params = json!({"options": [
            {"optionId": "allow-once"},
            {"optionId": "allow-always"},
            {"optionId": "reject-once"}
        ]});
        assert_eq!(pick_option(&params, "allow-once"), "allow-once");
        assert_eq!(pick_option(&params, "reject-once"), "reject-once");
        // An option list that no longer offers what we want still yields our
        // intent rather than whatever happens to be first.
        assert_eq!(
            pick_option(
                &json!({"options": [{"optionId": "allow-always"}]}),
                "reject-once"
            ),
            "reject-once"
        );
    }

    #[test]
    fn text_and_thinking_chunks_are_separated_and_only_text_is_kept() {
        let mut state = TurnState::default();
        let mut events = Vec::new();
        let mut sink = |event: StreamEvent<'_>| match event {
            StreamEvent::Text(text) => events.push(format!("text:{text}")),
            StreamEvent::Thinking(text) => events.push(format!("think:{text}")),
            StreamEvent::ProviderActivity { title, status, .. } => {
                events.push(format!("activity:{title}:{status}"))
            }
            _ => {}
        };
        let update = |kind: &str, text: &str| json!({"update": {"sessionUpdate": kind, "content": {"type": "text", "text": text}}});
        absorb_notification(
            "session/update",
            Some(&update("agent_thought_chunk", "hmm")),
            &mut state,
            &mut sink,
        );
        absorb_notification(
            "session/update",
            Some(&update("agent_message_chunk", "hello")),
            &mut state,
            &mut sink,
        );
        absorb_notification(
            "session/update",
            Some(&json!({"update": {
                "sessionUpdate": "tool_call",
                "toolCallId": "call-1",
                "title": "`ls`",
                "status": "pending"
            }})),
            &mut state,
            &mut sink,
        );
        // Cursor's IDE-only notifications are dropped rather than half-rendered.
        absorb_notification(
            "cursor/update_todos",
            Some(&json!({})),
            &mut state,
            &mut sink,
        );

        assert_eq!(state.text, "hello");
        assert_eq!(
            events,
            vec!["think:hmm", "text:hello", "activity:`ls`:pending"]
        );
    }
}
