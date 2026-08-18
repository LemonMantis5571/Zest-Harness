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
    catalogue, Completion, EffortPolicy, ModelSpec, Provider, ProviderCommandRequest,
    ProviderFileChangeRequest, ProviderInteractionHost, ProviderQuestionRequest,
    ProviderSessionRef, StreamEvent, SystemPrompt, TurnRequest, CODEX_KNOWN_MODELS,
};
use crate::anthropic::types::Usage;
use crate::auth::{detect_codex_cli, AuthStatus};
use crate::config::DEFAULT_CODEX_MODEL;
use crate::error::{HarnessError, Result, PROVIDER_MESSAGE_PREFIX};
use crate::thread::new_id;
use crate::tools::approval::ToolRisk;
use crate::tools::external_agent::{
    prepare_external_command, resolve_program, scrub_secret_environment,
    scrub_zest_secret_environment,
};

const APP_SERVER_ARGS: &[&str] = &["app-server", "--listen", "stdio://"];
const THREAD_SANDBOX_MODE: &str = "workspace-write";
const TURN_SANDBOX_POLICY_TYPE: &str = "workspaceWrite";
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
    /// Per-provider effort allow-list. Empty means the standard set.
    configured_efforts: Vec<String>,
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
        efforts: Vec<String>,
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
        let catalogue = catalogue(
            &default_model,
            &models,
            CODEX_KNOWN_MODELS,
            EffortPolicy::Standard(&efforts),
        );
        Ok(Self {
            id,
            root: root.into(),
            command,
            default_model,
            models: Arc::new(RwLock::new(catalogue)),
            configured_models: models,
            configured_efforts: efforts,
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
        let thread_params = thread_start_params(
            &self.root,
            &req.model,
            approval_policy,
            req.system.as_ref(),
            self.allow_mcp,
        );
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
                    "type": TURN_SANDBOX_POLICY_TYPE,
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
                catalogue(
                    &self.default_model,
                    &self.configured_models,
                    CODEX_KNOWN_MODELS,
                    EffortPolicy::Standard(&self.configured_efforts),
                )
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
            if let Some((fresh, cached)) = split_token_usage(&params) {
                state.usage.input_tokens = fresh;
                state.usage.cache_read_input_tokens = cached;
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
                return Err(codex_failure(&params, "Codex turn failed"));
            }
            return Ok(true);
        }
        "error" => {
            // `willRetry` means the app-server is handling it and more events are
            // coming. Ending the turn here would abort one that recovers.
            if params.get("willRetry").and_then(Value::as_bool) == Some(true) {
                let detail = codex_error_message(&params)
                    .unwrap_or_else(|| "retrying after a provider error".into());
                on_event(StreamEvent::ProviderActivity {
                    id: "codex-retry",
                    title: &detail,
                    status: "running",
                });
                return Ok(false);
            }
            return Err(codex_failure(&params, "Codex app-server error"));
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

fn thread_start_params(
    root: &std::path::Path,
    model: &str,
    approval_policy: &str,
    system: Option<&SystemPrompt>,
    allow_mcp: bool,
) -> Value {
    json!({
        "model": model,
        "cwd": root.to_string_lossy(),
        // `thread/start` uses the CLI-facing sandbox enum. The nested
        // `turn/start.sandboxPolicy.type` uses the app-server enum
        // (`workspaceWrite`) and is intentionally different.
        "sandbox": THREAD_SANDBOX_MODE,
        "approvalPolicy": approval_policy,
        "baseInstructions": system.map(SystemPrompt::text),
        "config": if allow_mcp { json!({}) } else { json!({"mcp_servers": {}}) },
    })
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

/// The failure Codex reported, as an error the front-ends can show verbatim.
///
/// Codex writes these for a person: `"You've hit your usage limit ... try again at
/// <date>"` names the cause *and* the fix. They are tagged
/// [`PROVIDER_MESSAGE_PREFIX`] so the desktop stops replacing them with a generic
/// sentence, and `codexErrorInfo` rides along in `kind` for the log.
fn codex_failure(params: &Value, fallback: &str) -> HarnessError {
    let message = codex_error_message(params).unwrap_or_else(|| fallback.to_string());
    let code = codex_error_object(params)
        .and_then(|error| string_field(error, &["codexErrorInfo", "code"]))
        .unwrap_or_else(|| "codex".to_string());
    HarnessError::Stream {
        kind: format!("{PROVIDER_MESSAGE_PREFIX}{code}"),
        message,
    }
}

/// Codex nests the human-readable reason, and it nests it in two places: an
/// `error` notification puts it at `params.error.message`, while `turn/completed`
/// puts it at `params.turn.error.message`.
///
/// The bug this fixes: both call sites used [`string_field`], which searches
/// *sibling keys* rather than a path. Given `error` as an object it found no
/// string, fell through to the placeholder, and discarded every actionable
/// message Codex has ever sent.
fn codex_error_message(params: &Value) -> Option<String> {
    codex_error_object(params)
        .and_then(|error| string_field(error, &["message", "detail"]))
        .or_else(|| {
            // Older/flatter shapes, still worth accepting.
            string_field(params, &["message", "error"])
        })
        .map(|message| message.trim().to_string())
        .filter(|message| !message.is_empty())
}

fn codex_error_object(params: &Value) -> Option<&Value> {
    for path in [&["error"][..], &["turn", "error"][..]] {
        let mut node = params;
        let found = path.iter().all(|key| match node.get(*key) {
            Some(next) => {
                node = next;
                true
            }
            None => false,
        });
        if found && node.is_object() {
            return Some(node);
        }
    }
    None
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

/// Split a Codex token-usage report into `(fresh input, cache reads)`.
///
/// Codex counts its cached prefix *inside* `inputTokens`, while the ledger
/// keeps cache reads in their own column the way the Messages API does. Without
/// taking the cached share back out, every Codex turn is filed as a total cache
/// miss and drags the headline hit rate down with it. `None` when the report
/// carries no input figure at all, so a silent turn stays silent rather than
/// being recorded as zero.
fn split_token_usage(params: &Value) -> Option<(u32, u32)> {
    let input = usage_number(params, &["inputTokens", "input_tokens"])?;
    // Clamped: a report claiming more cached than total would otherwise
    // underflow the subtraction.
    let cached = usage_number(params, &["cachedInputTokens", "cached_input_tokens"])
        .unwrap_or(0)
        .min(input);
    Some((bounded_u32(input - cached), bounded_u32(cached)))
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

    /// The reported failure: Codex said "You've hit your usage limit ... try again
    /// at Aug 19th" and the chat showed "The provider could not complete the
    /// request. Try again." The reason was nested one level deeper than the
    /// sibling-key lookup could see, so it was replaced by a placeholder.
    ///
    /// Payload copied from a live `codex app-server` session.
    #[test]
    fn a_usage_limit_reaches_the_user_in_the_provider_s_own_words() {
        let params = json!({
            "error": {
                "message": "You've hit your usage limit. Upgrade to Pro or try again at Aug 19th, 2026 10:04 PM.",
                "codexErrorInfo": "usageLimitExceeded",
                "additionalDetails": null
            },
            "willRetry": false,
            "threadId": "t",
            "turnId": "u"
        });

        let err = codex_failure(&params, "Codex app-server error");
        let shown = err
            .provider_user_message()
            .expect("a provider-authored reason must survive to the UI");
        assert!(shown.contains("usage limit"), "{shown}");
        assert!(
            shown.contains("Aug 19th"),
            "the retry time is the actionable half: {shown}"
        );
    }

    /// `turn/completed` nests it one level deeper again, under `turn`.
    #[test]
    fn a_failed_turn_reports_the_reason_from_under_the_turn_object() {
        let params = json!({
            "threadId": "t",
            "turn": {
                "id": "u",
                "status": "failed",
                "error": { "message": "model refused", "codexErrorInfo": "refusal" }
            }
        });

        assert_eq!(
            codex_failure(&params, "Codex turn failed").provider_user_message(),
            Some("model refused")
        );
    }

    /// With nothing to quote, the fallback is a placeholder — and it must *not*
    /// claim to be the provider's own words, or an internal string would be
    /// rendered as a chat message.
    #[test]
    fn an_error_with_no_message_still_fails_without_inventing_one() {
        let err = codex_failure(&json!({ "willRetry": false }), "Codex app-server error");
        assert_eq!(
            err.to_string(),
            "stream provider:codex: Codex app-server error"
        );
    }

    /// A flat `params.message` was the shape this code was written against. It
    /// still has to work — the nested lookup is an addition, not a replacement.
    #[test]
    fn a_flat_message_field_is_still_read() {
        assert_eq!(
            codex_error_message(&json!({ "message": "flat reason" })).as_deref(),
            Some("flat reason")
        );
    }

    #[test]
    fn thread_start_uses_the_cli_sandbox_enum() {
        // `thread/start.sandbox` is not the same enum as
        // `turn/start.sandboxPolicy.type`. The former uses kebab-case; sending
        // the latter here makes Codex reject every first turn before it can
        // reach the model.
        let params = thread_start_params(
            std::path::Path::new("C:/workspace"),
            "gpt-5.6-terra",
            "on-request",
            None,
            false,
        );
        assert_eq!(params["sandbox"], "workspace-write");
        assert_ne!(params["sandbox"], TURN_SANDBOX_POLICY_TYPE);
    }

    #[test]
    fn cached_input_is_taken_back_out_of_the_prompt_total() {
        // Codex nests the usage under `tokenUsage.last`, and counts the cached
        // prefix inside `inputTokens` rather than beside it.
        let params = json!({
            "tokenUsage": {
                "last": {
                    "inputTokens": 10_000,
                    "cachedInputTokens": 8_000,
                    "outputTokens": 250,
                }
            }
        });
        assert_eq!(split_token_usage(&params), Some((2_000, 8_000)));
    }

    #[test]
    fn a_report_without_a_cached_figure_is_all_fresh_input() {
        let params = json!({ "tokenUsage": { "last": { "inputTokens": 500 } } });
        assert_eq!(split_token_usage(&params), Some((500, 0)));
        assert_eq!(split_token_usage(&json!({})), None);
    }

    /// More cached than total is not physical, but it must not underflow into
    /// four billion fresh input tokens either.
    #[test]
    fn an_impossible_cached_figure_is_clamped_to_the_prompt() {
        let params = json!({ "inputTokens": 100, "cachedInputTokens": 900 });
        assert_eq!(split_token_usage(&params), Some((0, 100)));
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
            allow_tool_use: true,
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
            allow_tool_use: true,
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
