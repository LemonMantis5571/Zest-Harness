//! Native Codex app-server provider.
//!
//! Codex owns its authenticated account and its coding loop. Zest supplies the
//! workspace, parent instructions, approvals, and a durable thread reference;
//! it never reads or forwards Codex credentials.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::session::JsonlProcess;
use super::{
    catalogue_for_provider, Completion, ModelSpec, Provider, ProviderCommandRequest,
    ProviderFileChangeRequest, ProviderInteractionHost, ProviderQuestionRequest,
    ProviderSessionRef, StreamEvent, TurnRequest,
};
use crate::anthropic::types::Usage;
use crate::auth::{detect_codex_cli, AuthStatus};
use crate::config::DEFAULT_CODEX_MODEL;
use crate::error::{HarnessError, Result};
use crate::thread::new_id;
use crate::tools::approval::ToolRisk;
use crate::tools::external_agent::{
    prepare_external_command, resolve_program, scrub_secret_environment,
    scrub_zest_secret_environment,
};

const APP_SERVER_ARGS: &[&str] = &["app-server", "--listen", "stdio://"];
const CLIENT_NAME: &str = "zest";
const CLIENT_TITLE: &str = "Zest";
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct CodexAppServerProvider {
    id: String,
    root: PathBuf,
    command: String,
    default_model: String,
    models: Arc<RwLock<Vec<ModelSpec>>>,
    configured_models: Vec<String>,
    allow_mcp: bool,
    timeout_secs: u64,
    auth: AuthStatus,
}

impl CodexAppServerProvider {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        root: impl Into<PathBuf>,
        command: impl Into<String>,
        model: impl Into<String>,
        models: Vec<String>,
        allow_mcp: bool,
        timeout_secs: u64,
    ) -> Result<Self> {
        let command = command.into();
        if command.trim().is_empty() {
            return Err(HarnessError::Other(
                "Codex CLI command cannot be empty".into(),
            ));
        }
        if timeout_secs == 0 || timeout_secs > 3_600 {
            return Err(HarnessError::Other(
                "Codex CLI timeout_secs must be between 1 and 3600".into(),
            ));
        }
        let id = id.into();
        let default_model = model.into().trim().to_string();
        let default_model = if default_model.is_empty() {
            DEFAULT_CODEX_MODEL.to_string()
        } else {
            default_model
        };
        let catalogue = catalogue_for_provider(&id, &default_model, &models, &[]);
        Ok(Self {
            id,
            root: root.into(),
            command,
            default_model,
            models: Arc::new(RwLock::new(catalogue)),
            configured_models: models,
            allow_mcp,
            timeout_secs,
            auth: detect_codex_cli(),
        })
    }

    pub fn allow_mcp(&self) -> bool {
        self.allow_mcp
    }

    /// Ask a short-lived app-server for its model catalogue. Only a successful
    /// response is cached; a failed discovery leaves configured/built-in models
    /// intact.
    pub async fn discover_models(&self) -> Result<Vec<ModelSpec>> {
        let mut process = self.spawn().await?;
        let mut request_id = 1_u64;
        let _ = rpc_request(
            &mut process,
            &mut request_id,
            "initialize",
            json!({
                "clientInfo": {"name": CLIENT_NAME, "title": CLIENT_TITLE, "version": CLIENT_VERSION},
                "capabilities": {"experimentalApi": false}
            }),
            self.timeout(),
            None,
            None,
            &mut |_event| {},
        )
        .await?;
        process.send(&json!({"method":"initialized"})).await?;
        let response = rpc_request(
            &mut process,
            &mut request_id,
            "model/list",
            json!({"includeHidden": false}),
            self.timeout(),
            None,
            None,
            &mut |_event| {},
        )
        .await?;
        let models = parse_models(&response, &self.default_model);
        if models.is_empty() {
            return Err(HarnessError::Other(
                "Codex model/list returned no usable models".into(),
            ));
        }
        if let Ok(mut cached) = self.models.write() {
            *cached = models.clone();
        }
        let _ = process.kill().await;
        Ok(models)
    }

    fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs.max(1))
    }

    async fn spawn(&self) -> Result<JsonlProcess> {
        let args = APP_SERVER_ARGS
            .iter()
            .map(|arg| (*arg).to_string())
            .collect::<Vec<_>>();
        // Resolve before spawning: on Windows the Codex CLI is an npm `.cmd`
        // shim with no `.exe`, which a bare program name cannot reach.
        let mut command = tokio::process::Command::new(resolve_program(&self.command));
        command.args(&args).current_dir(&self.root);
        // Resolve the CLI against the user's current PATH, the same way the
        // Settings availability check does. Without this a desktop process
        // that predates the Codex install reports the CLI as available and
        // then fails to launch it.
        prepare_external_command(&mut command);
        // Codex authenticates through its own CLI-managed store, so Zest's
        // provider credentials are always withheld. When Codex is allowed to
        // run MCP servers those children need the user's own environment, so
        // only Zest's own keys are removed in that case.
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
        let mut process = self.spawn().await?;
        let mut request_id = 1_u64;
        rpc_request(
            &mut process,
            &mut request_id,
            "initialize",
            json!({
                "clientInfo": {"name": CLIENT_NAME, "title": CLIENT_TITLE, "version": CLIENT_VERSION},
                "capabilities": {"experimentalApi": false}
            }),
            self.timeout(),
            req.cancel.as_ref(),
            req.interaction.clone(),
            on_event,
        )
        .await?;
        process.send(&json!({"method":"initialized"})).await?;

        let approval_policy = if req.interaction.is_some() {
            "on-request"
        } else {
            "never"
        };
        let thread_params = json!({
            "model": &req.model,
            "cwd": self.root.to_string_lossy(),
            "sandbox": "workspaceWrite",
            "approvalPolicy": approval_policy,
            "baseInstructions": req.system,
            "config": if self.allow_mcp { json!({}) } else { json!({"mcp_servers": {}}) },
        });
        let mut resumed = false;
        let mut resume_failed = false;
        let requested_thread = match req.provider_session.as_ref() {
            Some(ProviderSessionRef::CodexAppServer { thread_id }) => {
                let response = rpc_request(
                    &mut process,
                    &mut request_id,
                    "thread/resume",
                    add_thread_id(thread_params.clone(), thread_id),
                    self.timeout(),
                    req.cancel.as_ref(),
                    req.interaction.clone(),
                    on_event,
                )
                .await;
                match response {
                    Ok(value) => match parse_thread_id(&value) {
                        Some(thread_id) => {
                            resumed = true;
                            thread_id
                        }
                        None => {
                            resume_failed = true;
                            String::new()
                        }
                    },
                    Err(_) => {
                        // A provider-native cursor is an optimization. If the
                        // server cannot resume it, the transcript remains the
                        // authority and we start a clean thread below.
                        resume_failed = true;
                        String::new()
                    }
                }
            }
            None => String::new(),
        };
        let thread_id = if requested_thread.is_empty() {
            let response = rpc_request(
                &mut process,
                &mut request_id,
                "thread/start",
                thread_params,
                self.timeout(),
                req.cancel.as_ref(),
                req.interaction.clone(),
                on_event,
            )
            .await?;
            parse_thread_id(&response).ok_or_else(|| HarnessError::Stream {
                kind: "codex_protocol".into(),
                message: "thread/start returned no thread id".into(),
            })?
        } else {
            requested_thread
        };

        if resume_failed {
            on_event(StreamEvent::ProviderActivity {
                id: "codex-session",
                title: "Restored from saved conversation",
                status: "restarted",
            });
        }
        let prompt = prompt_for_turn(req, req.provider_session.is_none() || !resumed);
        let turn_response = rpc_request(
            &mut process,
            &mut request_id,
            "turn/start",
            json!({
                "threadId": thread_id,
                "input": [{"type": "text", "text": prompt}],
                "model": &req.model,
                "effort": req.effort.as_deref().map(normalize_effort),
                "approvalPolicy": approval_policy,
                "cwd": self.root.to_string_lossy(),
                "sandboxPolicy": {
                    "type": "workspaceWrite",
                    "writableRoots": [self.root.to_string_lossy()],
                    "networkAccess": false,
                },
            }),
            self.timeout(),
            req.cancel.as_ref(),
            req.interaction.clone(),
            on_event,
        )
        .await?;
        let mut turn_id = parse_turn_id(&turn_response);
        let mut state = StreamState::default();

        loop {
            let message = match process.next(self.timeout(), req.cancel.as_ref()).await {
                Ok(Some(message)) => message,
                Ok(None) => return Err(HarnessError::PrematureEof),
                Err(HarnessError::Cancelled) => {
                    if let Some(turn_id) = turn_id.as_deref() {
                        let _ = process
                            .send(&json!({
                                "id": request_id,
                                "method": "turn/interrupt",
                                "params": {"threadId": thread_id, "turnId": turn_id}
                            }))
                            .await;
                    }
                    let _ = process.kill().await;
                    return Err(HarnessError::Cancelled);
                }
                Err(error) => return Err(error),
            };
            if is_response_for(&message, request_id.saturating_sub(1)) {
                if let Some(value) = message.get("result") {
                    turn_id = parse_turn_id(value).or(turn_id);
                }
                if message.get("error").is_some() {
                    return Err(protocol_error(&message));
                }
                continue;
            }
            if message.get("method").is_some() {
                let done = handle_message(
                    &mut process,
                    &message,
                    &mut state,
                    req.interaction.clone(),
                    on_event,
                )
                .await?;
                if done {
                    break;
                }
            }
        }

        if state.text.trim().is_empty() {
            return Err(HarnessError::Other(
                "Codex app-server returned no assistant answer".into(),
            ));
        }
        let completion = Completion {
            content: vec![json!({"type":"text", "text": state.text})],
            stop_reason: Some("end_turn".into()),
            usage: state.usage,
            usage_available: state.usage_available,
            limits: None,
            served_model: state.served_model,
            provider_session: Some(ProviderSessionRef::CodexAppServer { thread_id }),
        };
        let _ = turn_id;
        let _ = process.kill().await;
        Ok(completion)
    }
}

#[async_trait]
impl Provider for CodexAppServerProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn models(&self) -> Vec<ModelSpec> {
        self.models
            .read()
            .map(|models| models.clone())
            .unwrap_or_else(|_| {
                catalogue_for_provider(&self.id, &self.default_model, &self.configured_models, &[])
            })
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
        self.run_turn(req, on_event).await
    }
}

#[derive(Default)]
struct StreamState {
    text: String,
    usage: Usage,
    usage_available: bool,
    served_model: Option<String>,
}

#[allow(clippy::too_many_arguments)]
async fn rpc_request(
    process: &mut JsonlProcess,
    next_id: &mut u64,
    method: &str,
    params: Value,
    timeout: Duration,
    cancel: Option<&crate::cancel::CancelToken>,
    interaction: Option<Arc<dyn ProviderInteractionHost>>,
    on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
) -> Result<Value> {
    let id = *next_id;
    *next_id = (*next_id).saturating_add(1);
    process
        .send(&json!({"id":id,"method":method,"params":params}))
        .await?;
    loop {
        let Some(message) = process.next(timeout, cancel).await? else {
            return Err(HarnessError::PrematureEof);
        };
        if is_response_for(&message, id) {
            if message.get("error").is_some() {
                return Err(protocol_error(&message));
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
        if message.get("method").is_some() {
            let mut state = StreamState::default();
            let _ = handle_message(process, &message, &mut state, interaction.clone(), on_event)
                .await?;
        }
    }
}

async fn handle_message(
    process: &mut JsonlProcess,
    message: &Value,
    state: &mut StreamState,
    interaction: Option<Arc<dyn ProviderInteractionHost>>,
    on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
) -> Result<bool> {
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some(id) = message.get("id") {
        let result =
            server_request_result(method, message.get("params"), interaction, on_event).await;
        process.send(&json!({"id":id,"result":result})).await?;
        return Ok(false);
    }

    let params = message.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "item/agentMessage/delta" => {
            if let Some(delta) = string_field(&params, &["delta", "text"]) {
                state.text.push_str(&delta);
                on_event(StreamEvent::Text(&delta));
            }
        }
        "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
            if let Some(delta) = string_field(&params, &["delta", "text"]) {
                on_event(StreamEvent::Thinking(&delta));
            }
        }
        "item/commandExecution/outputDelta" | "item/fileChange/outputDelta" => {
            let id = string_field(&params, &["itemId"]).unwrap_or_else(|| "codex".into());
            on_event(StreamEvent::ProviderActivity {
                id: &id,
                title: method,
                status: "running",
            });
        }
        "model/rerouted" => {
            let requested =
                string_field(&params, &["requestedModel", "requested"]).unwrap_or_default();
            if let Some(served) = string_field(&params, &["servedModel", "served"]) {
                state.served_model = Some(served.clone());
                if !requested.is_empty() && requested != served {
                    on_event(StreamEvent::ModelSubstituted { requested, served });
                }
            }
        }
        "thread/tokenUsage/updated" => {
            if let Some(input) = usage_number(&params, &["inputTokens", "input_tokens"]) {
                state.usage.input_tokens = bounded_u32(input);
                state.usage_available = true;
            }
            if let Some(output) = usage_number(&params, &["outputTokens", "output_tokens"]) {
                state.usage.output_tokens = bounded_u32(output);
                state.usage_available = true;
            }
        }
        "turn/completed" => {
            let status = string_field(&params, &["status"]).or_else(|| {
                params
                    .get("turn")
                    .and_then(|turn| string_field(turn, &["status"]))
            });
            if status.as_deref().is_some_and(|status| {
                matches!(status, "failed" | "error" | "interrupted" | "cancelled")
            }) {
                return Err(HarnessError::Other(
                    string_field(&params, &["error", "message"])
                        .unwrap_or_else(|| "Codex turn failed".into()),
                ));
            }
            return Ok(true);
        }
        "error" => {
            return Err(HarnessError::Other(
                string_field(&params, &["message", "error"])
                    .unwrap_or_else(|| "Codex app-server error".into()),
            ));
        }
        _ => {}
    }
    Ok(false)
}

async fn server_request_result(
    method: &str,
    params: Option<&Value>,
    interaction: Option<Arc<dyn ProviderInteractionHost>>,
    on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
) -> Value {
    let params = params.cloned().unwrap_or(Value::Null);
    match method {
        "item/commandExecution/requestApproval" => {
            let approval_id = string_field(&params, &["approvalId", "id"])
                .unwrap_or_else(|| new_id("provider-approval"));
            let command = string_field(&params, &["command"]).unwrap_or_default();
            let cwd = string_field(&params, &["cwd"]);
            if let Some(host) = interaction.as_ref() {
                host.prepare_command_approval(&approval_id).await;
            }
            on_event(StreamEvent::ApprovalNeeded {
                approval_id: approval_id.clone(),
                tool_name: "codex_command".into(),
                tool_call_id: approval_id.clone(),
                risk: ToolRisk::Exec,
                path: cwd.clone().unwrap_or_default(),
                summary: command.clone(),
                diff: String::new(),
            });
            let approved = if let Some(host) = interaction {
                host.approve_command(ProviderCommandRequest {
                    approval_id,
                    command,
                    cwd,
                    reason: string_field(&params, &["reason"]),
                })
                .await
            } else {
                false
            };
            json!({"decision": if approved { "accept" } else { "decline" }})
        }
        "item/fileChange/requestApproval" => {
            let approval_id = string_field(&params, &["approvalId", "id"])
                .unwrap_or_else(|| new_id("provider-approval"));
            let path = string_field(&params, &["path", "filePath"]);
            let diff = string_field(&params, &["diff", "patch"]);
            if let Some(host) = interaction.as_ref() {
                host.prepare_file_change_approval(&approval_id).await;
            }
            on_event(StreamEvent::ApprovalNeeded {
                approval_id: approval_id.clone(),
                tool_name: "codex_file_change".into(),
                tool_call_id: approval_id.clone(),
                risk: ToolRisk::Write,
                path: path.clone().unwrap_or_default(),
                summary: "Codex requested a file change".into(),
                diff: diff.clone().unwrap_or_default(),
            });
            let approved = if let Some(host) = interaction {
                host.approve_file_change(ProviderFileChangeRequest {
                    approval_id,
                    path,
                    diff,
                    reason: string_field(&params, &["reason"]),
                })
                .await
            } else {
                false
            };
            json!({"decision": if approved { "accept" } else { "decline" }})
        }
        // These requests require a live UI and are deliberately denied when
        // the provider cannot express them through the shared host.
        "item/tool/requestUserInput" => {
            let question = parse_question_request(&params);
            let question_id = question.id.clone();
            if let Some(host) = interaction {
                host.prepare_question(&question_id).await;
                on_event(StreamEvent::QuestionNeeded {
                    question_id: question_id.clone(),
                    tool_call_id: question_id.clone(),
                    prompt: question.prompt.clone(),
                    choices: question.choices.clone(),
                    multiple: question.multiple,
                    placeholder: None,
                });
                if let Some(answers) = host.answer_question(question).await {
                    let mut answer_map = serde_json::Map::new();
                    answer_map.insert(question_id, json!({"answers": answers}));
                    return json!({"answers": answer_map});
                }
            }
            json!({"answers": {}})
        }
        _ => json!({"decision": "decline"}),
    }
}

fn add_thread_id(mut params: Value, thread_id: &str) -> Value {
    if let Some(object) = params.as_object_mut() {
        object.insert("threadId".into(), Value::String(thread_id.to_string()));
    }
    params
}

fn parse_thread_id(value: &Value) -> Option<String> {
    string_field(value, &["threadId", "id"]).or_else(|| {
        value
            .get("thread")
            .and_then(|thread| string_field(thread, &["id", "threadId"]))
    })
}

fn parse_turn_id(value: &Value) -> Option<String> {
    string_field(value, &["turnId", "id"]).or_else(|| {
        value
            .get("turn")
            .and_then(|turn| string_field(turn, &["id", "turnId"]))
    })
}

fn parse_models(value: &Value, default_model: &str) -> Vec<ModelSpec> {
    let items = value
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| value.get("models").and_then(Value::as_array))
        .cloned()
        .unwrap_or_default();
    let mut models = items
        .into_iter()
        .filter_map(|item| {
            let id = string_field(&item, &["id", "model"])?;
            let efforts = item
                .get("supportedReasoningEfforts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            Some(ModelSpec {
                context_window: item
                    .get("contextWindow")
                    .and_then(Value::as_u64)
                    .unwrap_or_else(|| super::context_window_for_model(&id)),
                id,
                efforts,
                supports_tools: true,
                supports_vision: false,
            })
        })
        .collect::<Vec<_>>();
    if models.is_empty() {
        return models;
    }
    if !models.iter().any(|model| model.id == default_model) {
        models.insert(
            0,
            ModelSpec {
                id: default_model.to_string(),
                efforts: super::STANDARD_EFFORTS
                    .iter()
                    .map(|effort| (*effort).into())
                    .collect(),
                context_window: super::context_window_for_model(default_model),
                supports_tools: true,
                supports_vision: false,
            },
        );
    }
    models
}

fn prompt_for_turn(req: &TurnRequest, include_history: bool) -> String {
    let mut output = String::new();
    if include_history && req.messages.len() > 1 {
        output.push_str("Conversation context from Zest:\n\n");
        for message in &req.messages[..req.messages.len() - 1] {
            output.push_str(if message.role == "assistant" {
                "Assistant: "
            } else {
                "User: "
            });
            output.push_str(&message_text(&message.content));
            output.push_str("\n\n");
        }
    }
    if let Some(message) = req.messages.last() {
        output.push_str(&message_text(&message.content));
    }
    output
}

fn message_text(content: &[Value]) -> String {
    content
        .iter()
        .filter_map(render_message_block)
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_message_block(block: &Value) -> Option<String> {
    if let Some(text) = string_field(block, &["text"]) {
        return Some(text);
    }
    if let Some(text) = block.as_str() {
        return Some(text.to_string());
    }

    match block.get("type").and_then(Value::as_str) {
        Some("tool_use") => {
            let name = string_field(block, &["name"]).unwrap_or_else(|| "tool".into());
            let input = block
                .get("input")
                .map(compact_json)
                .unwrap_or_else(|| "{}".into());
            Some(format!("[Tool call: {name}]\nInput: {input}"))
        }
        Some("tool_result") => {
            let label = if block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                " (error)"
            } else {
                ""
            };
            let content = block
                .get("content")
                .map(render_nested_content)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "[empty]".into());
            Some(format!("[Tool result{label}]\n{content}"))
        }
        Some("thinking") => None,
        _ => block
            .get("content")
            .map(render_nested_content)
            .filter(|value| !value.is_empty()),
    }
}

fn render_nested_content(value: &Value) -> String {
    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(render_message_block)
            .collect::<Vec<_>>()
            .join("\n"),
        Value::String(text) => text.clone(),
        _ => compact_json(value),
    }
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "[unserializable]".into())
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str).map(str::to_string))
}

fn find_number(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_u64))
}

fn usage_number(value: &Value, keys: &[&str]) -> Option<u64> {
    let usage = value.get("tokenUsage").unwrap_or(value);
    let preferred = usage
        .get("last")
        .or_else(|| usage.get("total"))
        .unwrap_or(usage);
    find_number(preferred, keys).or_else(|| find_number(value, keys))
}

fn parse_question_request(params: &Value) -> ProviderQuestionRequest {
    let question = params
        .get("questions")
        .and_then(Value::as_array)
        .and_then(|questions| questions.first())
        .unwrap_or(params);
    let id = string_field(question, &["id", "questionId", "itemId"])
        .or_else(|| string_field(params, &["questionId", "itemId"]))
        .unwrap_or_else(|| "codex-question".into());
    let prompt = string_field(question, &["question", "prompt", "header"])
        .or_else(|| string_field(params, &["question", "prompt"]))
        .unwrap_or_else(|| "Codex requested input".into());
    let choices = question
        .get("options")
        .or_else(|| question.get("choices"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .or_else(|| string_field(item, &["label", "value", "id"]))
                })
                .collect()
        })
        .unwrap_or_default();
    let multiple = question
        .get("multiple")
        .and_then(Value::as_bool)
        .or_else(|| params.get("multiple").and_then(Value::as_bool))
        .unwrap_or(false);
    ProviderQuestionRequest {
        id,
        prompt,
        choices,
        multiple,
    }
}

fn bounded_u32(value: u64) -> u32 {
    value.min(u64::from(u32::MAX)) as u32
}

fn normalize_effort(value: &str) -> String {
    match value {
        "extra" | "extra_high" => "xhigh".into(),
        other => other.to_string(),
    }
}

fn is_response_for(message: &Value, id: u64) -> bool {
    message.get("id").and_then(Value::as_u64) == Some(id)
}

fn protocol_error(message: &Value) -> HarnessError {
    let error = message.get("error").cloned().unwrap_or(Value::Null);
    HarnessError::Stream {
        kind: "codex_protocol".into(),
        message: string_field(&error, &["message"]).unwrap_or_else(|| error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::types::Message;

    #[test]
    fn model_list_keeps_configured_default_when_server_omits_it() {
        let models = parse_models(
            &json!({
                "data": [{
                    "id": "gpt-5.6-terra",
                    "contextWindow": 400_000,
                    "supportedReasoningEfforts": ["low", "high"]
                }]
            }),
            "gpt-5.6-sol",
        );

        assert_eq!(models[0].id, "gpt-5.6-sol");
        assert!(models.iter().any(|model| model.id == "gpt-5.6-terra"));
        assert_eq!(models[1].context_window, 400_000);
    }

    #[test]
    fn resumed_turn_prompt_contains_only_the_new_user_input() {
        let request = TurnRequest {
            model: "gpt-5.6-sol".into(),
            system: None,
            messages: vec![
                Message::user_text("Earlier request"),
                Message::assistant(vec![json!({"type": "text", "text": "Earlier answer"})]),
                Message::user_text("New request"),
            ],
            tools: Vec::new(),
            max_tokens: 100,
            effort: Some("high".into()),
            thinking: true,
            provider_session: Some(ProviderSessionRef::CodexAppServer {
                thread_id: "thread-1".into(),
            }),
            interaction: None,
            cancel: None,
        };

        let prompt = prompt_for_turn(&request, false);
        assert_eq!(prompt, "New request");
        assert!(!prompt.contains("Earlier answer"));
    }

    #[test]
    fn fallback_prompt_preserves_tool_calls_and_results() {
        let request = TurnRequest {
            model: "gpt-5.6-sol".into(),
            system: None,
            messages: vec![
                Message::user_text("Inspect the project."),
                Message::assistant(vec![json!({
                    "type": "tool_use",
                    "id": "call-1",
                    "name": "read_file",
                    "input": {"path": "src/lib.rs"}
                })]),
                Message::user_blocks(vec![json!({
                    "type": "tool_result",
                    "tool_use_id": "call-1",
                    "content": [{"type": "text", "text": "pub fn run() {}"}]
                })]),
                Message::user_text("Now summarize it."),
            ],
            tools: Vec::new(),
            max_tokens: 100,
            effort: Some("high".into()),
            thinking: true,
            provider_session: None,
            interaction: None,
            cancel: None,
        };

        let prompt = prompt_for_turn(&request, true);
        assert!(prompt.contains("[Tool call: read_file]"));
        assert!(prompt.contains("src/lib.rs"));
        assert!(prompt.contains("[Tool result]"));
        assert!(prompt.contains("pub fn run() {}"));
    }

    #[test]
    fn provider_session_reference_contains_only_a_thread_id() {
        let value = serde_json::to_value(ProviderSessionRef::CodexAppServer {
            thread_id: "thread-1".into(),
        })
        .unwrap();
        assert_eq!(
            value,
            json!({"kind": "codex_app_server", "thread_id": "thread-1"})
        );
    }

    #[test]
    fn user_input_requests_keep_question_ids_and_choices() {
        let question = parse_question_request(&json!({
            "questions": [{
                "id": "question-1",
                "question": "Which package manager should I use?",
                "options": [
                    {"label": "npm", "description": "Use npm"},
                    {"label": "pnpm", "description": "Use pnpm"}
                ],
                "multiple": true
            }]
        }));

        assert_eq!(question.id, "question-1");
        assert_eq!(question.prompt, "Which package manager should I use?");
        assert_eq!(question.choices, ["npm", "pnpm"]);
        assert!(question.multiple);
    }

    #[test]
    fn nested_token_usage_prefers_the_last_turn() {
        let value = json!({
            "tokenUsage": {
                "total": {"inputTokens": 900, "outputTokens": 120},
                "last": {"inputTokens": 90, "outputTokens": 12}
            }
        });

        assert_eq!(usage_number(&value, &["inputTokens"]), Some(90));
        assert_eq!(usage_number(&value, &["outputTokens"]), Some(12));
    }
}
