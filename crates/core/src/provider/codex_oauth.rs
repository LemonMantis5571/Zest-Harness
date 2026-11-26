//! ChatGPT Codex parent that owns Zest's agent loop.
//!
//! Talks to the unpublished Codex Responses backend. Identify as Zest; do not
//! impersonate the official CLI.

use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};

use super::{
    catalogue, Completion, EffortPolicy, ModelSpec, Provider, StreamEvent, SystemPrompt,
    TurnRequest, CODEX_KNOWN_MODELS, STREAM_INTERRUPTED,
};
use crate::anthropic::sse::SseParser;
use crate::anthropic::types::{Message, ToolDef, Usage};
use crate::auth::AuthStatus;
use crate::cancel::{wait_cancel, CancelToken};
use crate::codex_oauth::{refresh_and_store, CodexOAuthSession, BACKEND_URL, ORIGINATOR};
use crate::error::{HarnessError, Result};
use crate::retry::{self, AttemptError, RetryPolicy};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_INSTRUCTIONS: &str = "You are a helpful coding assistant.";

pub struct CodexOAuthProvider {
    id: String,
    account: String,
    client: reqwest::Client,
    session: Mutex<CodexOAuthSession>,
    default_model: String,
    models: Vec<ModelSpec>,
}

impl CodexOAuthProvider {
    pub fn new(
        id: impl Into<String>,
        account: impl Into<String>,
        session: CodexOAuthSession,
        default_model: impl Into<String>,
        models: Vec<ModelSpec>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            id: id.into(),
            account: account.into(),
            client,
            session: Mutex::new(session),
            default_model: default_model.into(),
            models,
        }
    }

    pub fn from_key(
        id: impl Into<String>,
        account: impl Into<String>,
        key: Option<String>,
        default_model: impl Into<String>,
        models: Vec<ModelSpec>,
    ) -> std::result::Result<Self, String> {
        let raw = key.ok_or_else(|| "ChatGPT sign-in is missing.".to_string())?;
        let session = CodexOAuthSession::parse_json(&raw)?;
        Ok(Self::new(id, account, session, default_model, models))
    }
}

#[async_trait]
impl Provider for CodexOAuthProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn models(&self) -> Vec<ModelSpec> {
        if self.models.is_empty() {
            catalogue(
                &self.default_model,
                &[],
                CODEX_KNOWN_MODELS,
                EffortPolicy::Standard(&[]),
            )
        } else {
            self.models.clone()
        }
    }

    fn auth_status(&self) -> AuthStatus {
        AuthStatus::Ready { account: None }
    }

    fn owns_agent_loop(&self) -> bool {
        false
    }

    async fn stream_turn(
        &self,
        req: &TurnRequest,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    ) -> Result<Completion> {
        let session = self
            .session
            .lock()
            .map_err(|_| HarnessError::Other("ChatGPT session lock poisoned".into()))?
            .clone();
        let session = refresh_and_store(&self.account, session)
            .await
            .map_err(HarnessError::Other)?;
        if let Ok(mut guard) = self.session.lock() {
            *guard = session.clone();
        }

        stream_responses_with_retry(&self.client, &session, req, on_event).await
    }
}

pub fn request_body(
    model: &str,
    effort: Option<&str>,
    system: Option<&SystemPrompt>,
    messages: &[Message],
    tools: &[ToolDef],
) -> Value {
    let (instructions, input) = responses_input(system, messages);
    let mut root = json!({
        "model": model,
        "input": input,
        "include": ["reasoning.encrypted_content"],
        "store": false,
        "stream": true,
    });
    if !instructions.is_empty() {
        root["instructions"] = json!(instructions);
    }
    if let Some(effort) = effort.filter(|value| !value.is_empty()) {
        root["reasoning"] = json!({ "effort": effort });
    }
    if !tools.is_empty() {
        root["tools"] = Value::Array(
            tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.input_schema,
                    })
                })
                .collect(),
        );
    }
    root
}

pub fn responses_input(system: Option<&SystemPrompt>, messages: &[Message]) -> (String, Value) {
    let mut instructions = system.map(SystemPrompt::text).unwrap_or_default();
    let mut input = Vec::new();
    for message in messages {
        let mut text = String::new();
        let mut user_parts = Vec::new();
        let mut has_user_image = false;
        let mut reasoning_items = Vec::new();
        let mut tool_calls = Vec::new();
        let mut tool_results = Vec::new();
        for block in &message.content {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let s = block.get("text").and_then(Value::as_str).unwrap_or("");
                    text.push_str(s);
                    if !s.is_empty() {
                        user_parts.push(json!({ "type": "input_text", "text": s }));
                    }
                }
                Some("image") => {
                    if let Some(url) = image_data_url(block) {
                        has_user_image = true;
                        user_parts.push(json!({ "type": "input_image", "image_url": url }));
                    }
                }
                Some("reasoning_encrypted") => {
                    if let (Some(id), Some(data)) = (json_str(block, "id"), json_str(block, "data"))
                    {
                        let summary = block
                            .get("summary")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        reasoning_items.push(json!({
                            "type": "reasoning",
                            "id": id,
                            "summary": summary,
                            "encrypted_content": data,
                        }));
                    }
                }
                Some("tool_use") => tool_calls.push(block),
                Some("tool_result") => tool_results.push(block),
                _ => {}
            }
        }
        if message.role == "system" {
            if !text.is_empty() {
                if !instructions.is_empty() {
                    instructions.push_str("\n\n");
                }
                instructions.push_str(&text);
            }
            continue;
        }
        if !tool_results.is_empty() {
            for result in tool_results {
                let Some(call_id) = json_str(result, "tool_use_id") else {
                    continue;
                };
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": format_tool_result_output(result.get("content")),
                }));
            }
            continue;
        }
        if message.role == "assistant" && (!tool_calls.is_empty() || !reasoning_items.is_empty()) {
            input.extend(reasoning_items);
            if !text.is_empty() {
                input.push(message_item("assistant", "output_text", &text));
            }
            for call in tool_calls {
                let Some(call_id) = json_str(call, "id") else {
                    continue;
                };
                let arguments = call
                    .get("input")
                    .map(|value| {
                        if value.is_string() {
                            value.as_str().unwrap_or("{}").to_string()
                        } else {
                            value.to_string()
                        }
                    })
                    .unwrap_or_else(|| "{}".into());
                let mut item = json!({
                    "type": "function_call",
                    "call_id": call_id,
                    "name": call.get("name").and_then(Value::as_str).unwrap_or(""),
                    "arguments": arguments,
                });
                if let Some(item_id) = json_str(call, "item_id").filter(|id| id.starts_with("fc_"))
                {
                    item["id"] = json!(item_id);
                }
                input.push(item);
            }
            continue;
        }
        if message.role != "assistant" && has_user_image {
            input.push(json!({
                "type": "message",
                "role": &message.role,
                "content": user_parts,
            }));
            continue;
        }
        let part_type = if message.role == "assistant" {
            "output_text"
        } else {
            "input_text"
        };
        input.push(message_item(&message.role, part_type, &text));
    }
    (instructions, Value::Array(input))
}

fn image_data_url(block: &Value) -> Option<String> {
    let source = block.get("source")?.as_object()?;
    if source.get("type")?.as_str()? != "base64" {
        return None;
    }
    let media_type = source.get("media_type")?.as_str()?.trim();
    let data = source.get("data")?.as_str()?.trim();
    if media_type.is_empty() || data.is_empty() {
        return None;
    }
    Some(format!("data:{media_type};base64,{data}"))
}

fn format_tool_result_output(content: Option<&Value>) -> Value {
    match content {
        Some(Value::String(text)) => Value::String(text.clone()),
        Some(Value::Array(blocks)) => {
            let has_image = blocks
                .iter()
                .any(|b| b.get("type").and_then(Value::as_str) == Some("image"));
            if !has_image {
                let joined = blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|b| b.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n");
                return Value::String(joined);
            }
            let mut parts = Vec::with_capacity(blocks.len());
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            parts.push(json!({ "type": "input_text", "text": text }));
                        }
                    }
                    Some("image") => {
                        if let Some(url) = image_data_url(block) {
                            parts.push(json!({ "type": "input_image", "image_url": url }));
                        }
                    }
                    _ => {}
                }
            }
            Value::Array(parts)
        }
        _ => Value::String(String::new()),
    }
}

fn message_item(role: &str, part_type: &str, text: &str) -> Value {
    json!({
        "type": "message",
        "role": role,
        "content": [{ "type": part_type, "text": text }],
    })
}

struct ResponsesAttempt<'a> {
    client: &'a reqwest::Client,
    session: &'a CodexOAuthSession,
    body: Value,
    cancel: Option<&'a CancelToken>,
}

#[async_trait]
impl retry::Attempt for ResponsesAttempt<'_> {
    async fn run(
        &mut self,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    ) -> std::result::Result<Completion, AttemptError> {
        tokio::select! {
            biased;
            _ = wait_cancel(self.cancel) => Err((HarnessError::Cancelled, None)),
            result = stream_responses_once(self.client, self.session, &self.body, on_event) => result,
        }
    }
}

async fn stream_responses_with_retry(
    client: &reqwest::Client,
    session: &CodexOAuthSession,
    req: &TurnRequest,
    on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
) -> Result<Completion> {
    let tools: &[ToolDef] = if req.allow_tool_use { &req.tools } else { &[] };
    let mut body = request_body(
        &req.model,
        req.effort.as_deref(),
        req.system.as_ref(),
        &req.messages,
        tools,
    );
    // ChatGPT's Codex Responses backend requires a non-empty `instructions`
    // field even on minimal probe turns that carry no system prompt.
    if body.get("instructions").is_none() {
        body["instructions"] = json!(DEFAULT_INSTRUCTIONS);
    }
    let mut attempt = ResponsesAttempt {
        client,
        session,
        body,
        cancel: req.cancel.as_ref(),
    };
    retry::run_with_retry(
        &RetryPolicy::default(),
        &mut attempt,
        on_event,
        req.cancel.as_ref(),
    )
    .await
}

async fn stream_responses_once(
    client: &reqwest::Client,
    session: &CodexOAuthSession,
    body: &Value,
    on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
) -> std::result::Result<Completion, AttemptError> {
    let response = tokio::time::timeout(STREAM_IDLE_TIMEOUT, send_once(client, session, body))
        .await
        .map_err(|_| (HarnessError::StreamIdleTimeout, None))??;
    let mut parser = SseParser::default();
    let mut body_stream = response.bytes_stream();
    let mut accumulator = ResponsesAccumulator::default();
    let mut idle_deadline = tokio::time::Instant::now() + STREAM_IDLE_TIMEOUT;
    let failure = 'read: loop {
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(idle_deadline) => break Some(HarnessError::StreamIdleTimeout),
            chunk = body_stream.next() => match chunk {
                Some(Ok(bytes)) => {
                    for payload in parser.feed(&bytes) {
                        let trimmed = payload.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        if trimmed == "[DONE]" {
                            break 'read None;
                        }
                        let event: Value = serde_json::from_str(trimmed).map_err(|err| (HarnessError::Json(err), None))?;
                        if let Err(error) = accumulator.push(&event, on_event) {
                            break 'read Some(error);
                        }
                        idle_deadline = tokio::time::Instant::now() + STREAM_IDLE_TIMEOUT;
                        if accumulator.done { break 'read None; }
                    }
                }
                Some(Err(error)) => break Some(HarnessError::Http(error)),
                None => break None,
            },
        }
    };
    accumulator
        .finish_stream(failure, on_event)
        .map_err(|err| (err, None))
}

async fn send_once(
    client: &reqwest::Client,
    session: &CodexOAuthSession,
    body: &Value,
) -> std::result::Result<reqwest::Response, AttemptError> {
    let response = client
        .post(format!("{BACKEND_URL}/responses"))
        .header("authorization", format!("Bearer {}", session.access_token))
        .header("ChatGPT-Account-ID", &session.account_id)
        .header("originator", ORIGINATOR)
        .header("accept", "text/event-stream")
        .header("content-type", "application/json")
        .json(body)
        .send()
        .await
        .map_err(|error| (HarnessError::Http(error), None))?;
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let retry_after = retry::retry_after_from_headers(response.headers());
    let body = response.text().await.unwrap_or_default();
    Err((HarnessError::Api { status, body }, retry_after))
}

#[derive(Default)]
struct ResponsesAccumulator {
    text: String,
    reasoning_items: Vec<Value>,
    tools: Vec<ToolAccum>,
    usage: Usage,
    usage_available: bool,
    served_model: Option<String>,
    incomplete_max_tokens: bool,
    done: bool,
}

#[derive(Default)]
struct ToolAccum {
    id: String,
    item_id: String,
    name: String,
    arguments: String,
}

impl ResponsesAccumulator {
    fn push(
        &mut self,
        event: &Value,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    ) -> Result<()> {
        if let Some(error) = event.get("error").filter(|value| !value.is_null()) {
            return Err(chatgpt_stream_error(error));
        }
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        if self.served_model.is_none() {
            self.served_model = event
                .pointer("/response/model")
                .or_else(|| event.get("model"))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
        }
        match kind {
            "error" => return Err(chatgpt_stream_error(event)),
            "response.output_text.delta"
            | "response.reasoning_text.delta"
            | "response.reasoning_summary_text.delta" => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    if kind == "response.output_text.delta" {
                        self.text.push_str(delta);
                        on_event(StreamEvent::Text(delta));
                    } else if !delta.is_empty() {
                        on_event(StreamEvent::Thinking(delta));
                    }
                }
            }
            "response.output_item.added" | "response.output_item.done" => {
                if let Some(item) = event.get("item") {
                    self.ingest_item(item, on_event);
                }
            }
            "response.function_call_arguments.delta" => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    if let Some(tool) =
                        self.tool_mut(json_str(event, "call_id"), json_str(event, "item_id"))
                    {
                        tool.arguments.push_str(delta);
                    }
                }
            }
            "response.function_call_arguments.done" => {
                if let Some(arguments) = json_str(event, "arguments") {
                    if let Some(tool) =
                        self.tool_mut(json_str(event, "call_id"), json_str(event, "item_id"))
                    {
                        tool.arguments = arguments.to_string();
                    }
                }
            }
            "response.failed" | "response.incomplete" | "response.completed" | "response.done" => {
                if let Some(error) = event.pointer("/response/error").filter(|v| !v.is_null()) {
                    return Err(chatgpt_stream_error(error));
                }
                if kind == "response.failed" {
                    return Err(HarnessError::from_provider_stream(
                        "error",
                        "ChatGPT stopped the reply before it finished.",
                    ));
                }
                let incomplete = kind == "response.incomplete"
                    || event.pointer("/response/status").and_then(Value::as_str)
                        == Some("incomplete");
                if incomplete {
                    if event
                        .pointer("/response/incomplete_details/reason")
                        .and_then(Value::as_str)
                        != Some("max_output_tokens")
                    {
                        return Err(HarnessError::StoppedEarly(
                            "ChatGPT marked the response incomplete".into(),
                        ));
                    }
                    self.incomplete_max_tokens = true;
                    // Only dispatch calls the terminal response confirms finished.
                    self.tools.clear();
                }
                if let Some(items) = event.pointer("/response/output").and_then(Value::as_array) {
                    for item in items {
                        if incomplete
                            && item["type"] == "function_call"
                            && item["status"] != "completed"
                        {
                            continue;
                        }
                        self.ingest_item(item, on_event);
                    }
                }
                if let Some(usage) = event
                    .pointer("/response/usage")
                    .or_else(|| event.get("usage"))
                {
                    self.take_usage(usage);
                }
                self.done = true;
            }
            _ => {
                if let Some(usage) = event.get("usage") {
                    self.take_usage(usage);
                }
            }
        }
        Ok(())
    }

    fn ingest_item(
        &mut self,
        item: &Value,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    ) {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                let call_id = json_str(item, "call_id");
                let Some(tool) = self.tool_mut(call_id, json_str(item, "id")) else {
                    return;
                };
                if tool.name.is_empty() {
                    if let Some(name) = json_str(item, "name") {
                        tool.name = name.to_string();
                    }
                }
                let arguments = item.get("arguments").and_then(Value::as_str).unwrap_or("");
                if !arguments.is_empty() {
                    tool.arguments = arguments.to_string();
                }
            }
            Some("reasoning") => {
                if let (Some(id), Some(encrypted)) =
                    (json_str(item, "id"), json_str(item, "encrypted_content"))
                {
                    if !self
                        .reasoning_items
                        .iter()
                        .any(|existing| json_str(existing, "id") == Some(id))
                    {
                        let summary = item
                            .get("summary")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        self.reasoning_items.push(json!({
                            "type": "reasoning_encrypted",
                            "id": id,
                            "summary": summary,
                            "data": encrypted,
                        }));
                    }
                }
            }
            Some("message") => {
                if let Some(parts) = item.get("content").and_then(Value::as_array) {
                    if self.text.is_empty() {
                        for part in parts {
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                if !text.is_empty() {
                                    self.text.push_str(text);
                                    on_event(StreamEvent::Text(text));
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn take_usage(&mut self, usage: &Value) {
        if usage.is_null() {
            return;
        }
        // Cached tokens are a subset of `input_tokens` on the Responses wire,
        // the same way they sit inside `prompt_tokens` on Chat Completions.
        // Left unsplit, every Codex OAuth cache hit is filed as fresh input
        // and the Profile tile reads as a lifetime miss.
        if let Some(prompt) = usage
            .get("input_tokens")
            .or_else(|| usage.get("prompt_tokens"))
            .and_then(Value::as_u64)
        {
            let cached = usage
                .pointer("/input_tokens_details/cached_tokens")
                .or_else(|| usage.pointer("/prompt_tokens_details/cached_tokens"))
                .or_else(|| usage.get("cached_tokens"))
                .or_else(|| usage.get("prompt_cache_hit_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(prompt);
            self.usage.input_tokens = bounded_u32(prompt - cached);
            self.usage.cache_read_input_tokens = bounded_u32(cached);
        }
        if let Some(output) = usage
            .get("output_tokens")
            .or_else(|| usage.get("completion_tokens"))
            .and_then(Value::as_u64)
        {
            self.usage.output_tokens = bounded_u32(output);
        }
        self.usage_available = true;
    }

    /// ChatGPT keys argument deltas by `item_id` (`fc_…`) and the call itself
    /// by `call_id`. Those must become one tool; a leftover keyed only by the
    /// item id is what went back as `call_id: ""`.
    fn tool_mut(&mut self, call_id: Option<&str>, item_id: Option<&str>) -> Option<&mut ToolAccum> {
        if call_id.is_none() && item_id.is_none() {
            return None;
        }
        let by_call = call_id.and_then(|id| self.tools.iter().position(|tool| tool.id == id));
        let by_item = item_id.and_then(|id| self.tools.iter().position(|tool| tool.item_id == id));
        let index = match (by_call, by_item) {
            (Some(a), Some(b)) if a != b => {
                let orphan = self.tools.remove(a.max(b));
                merge_tool(&mut self.tools[a.min(b)], orphan);
                a.min(b)
            }
            (Some(index), _) | (_, Some(index)) => index,
            (None, None) => {
                self.tools.push(ToolAccum::default());
                self.tools.len() - 1
            }
        };
        let tool = &mut self.tools[index];
        if let Some(call) = call_id {
            tool.id = call.to_string();
        } else if tool.id.is_empty() {
            tool.id = item_id.unwrap_or("").to_string();
        }
        if let Some(item) = item_id {
            if tool.item_id.is_empty() {
                tool.item_id = item.to_string();
            }
        }
        Some(tool)
    }

    fn finish_stream(
        mut self,
        failure: Option<HarnessError>,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    ) -> Result<Completion> {
        if let Some(error) = failure.or_else(|| (!self.done).then_some(HarnessError::PrematureEof))
        {
            if !error.is_retryable_stream_failure() || self.text.trim().is_empty() {
                return Err(error);
            }
            self.tools.clear();
            self.reasoning_items.clear();
            let mut partial = self.finish();
            partial.stop_reason = Some(STREAM_INTERRUPTED.into());
            return Ok(partial);
        }
        let completion = self.finish();
        // Publish only committed calls. Retrying a dropped stream must not
        // leave a running row for a tool the agent will never execute.
        for call in &completion.content {
            if call["type"] == "tool_use" {
                if let (Some(name), Some(id)) = (json_str(call, "name"), json_str(call, "id")) {
                    on_event(StreamEvent::ToolCallStart { name, id });
                }
            }
        }
        Ok(completion)
    }

    fn finish(self) -> Completion {
        let mut content = self.reasoning_items;
        if !self.text.is_empty() {
            content.push(json!({"type":"text", "text": self.text}));
        }
        let tools: Vec<ToolAccum> = self
            .tools
            .into_iter()
            .filter(|tool| !tool.id.is_empty() && !tool.name.is_empty())
            .collect();
        let stop_reason = if self.incomplete_max_tokens {
            Some("max_tokens".into())
        } else if !tools.is_empty() {
            Some("tool_use".into())
        } else {
            Some("end_turn".into())
        };
        for tool in tools {
            let trimmed = tool.arguments.trim();
            let input = if trimmed.is_empty() {
                json!({})
            } else {
                serde_json::from_str(trimmed)
                    .unwrap_or_else(|_| Value::String(tool.arguments.clone()))
            };
            let mut block = json!({
                "type": "tool_use",
                "id": tool.id,
                "name": tool.name,
                "input": input
            });
            if tool.item_id.starts_with("fc_") {
                block["item_id"] = json!(tool.item_id);
            }
            content.push(block);
        }
        Completion {
            content,
            stop_reason,
            usage: self.usage,
            usage_available: self.usage_available,
            limits: None,
            served_model: self.served_model,
            provider_session: None,
        }
    }
}

fn json_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn merge_tool(dest: &mut ToolAccum, src: ToolAccum) {
    if dest.id.is_empty() {
        dest.id = src.id;
    }
    if dest.item_id.is_empty() {
        dest.item_id = src.item_id;
    }
    if dest.name.is_empty() {
        dest.name = src.name;
    }
    if src.arguments.len() > dest.arguments.len() {
        dest.arguments = src.arguments;
    }
}

/// ChatGPT's own stream text, tagged so the desktop can show it. A missing
/// message stays untagged: that fallback is ours, not theirs.
fn chatgpt_stream_error(error: &Value) -> HarnessError {
    if let Some(message) = error
        .as_str()
        .map(str::trim)
        .filter(|message| !message.is_empty())
    {
        return HarnessError::from_provider_stream("error", message);
    }
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let kind = error
        .get("type")
        .or_else(|| error.get("code"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("error");
    if message.is_empty() {
        return HarnessError::Stream {
            kind: kind.to_string(),
            message: "ChatGPT Codex stream failed".into(),
        };
    }
    HarnessError::from_provider_stream(kind, message)
}

/// Saturate rather than wrap. A provider that reports a nonsense token count
/// should show as an implausibly large one, not as a small one.
fn bounded_u32(value: u64) -> u32 {
    value.min(u64::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::types::Message;

    fn msg(role: &str, content: Vec<Value>) -> Message {
        Message {
            role: role.into(),
            content,
        }
    }

    #[test]
    fn does_not_own_the_agent_loop() {
        let provider = CodexOAuthProvider::new(
            "codex",
            "codex",
            CodexOAuthSession {
                access_token: "a".into(),
                refresh_token: "r".into(),
                id_token: None,
                account_id: "acct".into(),
                expires_at: 9_999_999_999,
            },
            "gpt-5.6-sol",
            Vec::new(),
        );
        assert!(!provider.owns_agent_loop());
    }

    #[test]
    fn rewrites_messages_into_responses_input() {
        let system = SystemPrompt::new("base");
        let messages = vec![
            msg("user", vec![json!({"type":"text","text":"hi"})]),
            msg(
                "assistant",
                vec![
                    json!({"type":"text","text":"calling"}),
                    json!({"type":"tool_use","id":"c1","name":"read_file","input":{"path":"a.rs"}}),
                ],
            ),
            msg(
                "user",
                vec![json!({"type":"tool_result","tool_use_id":"c1","content":"ok"})],
            ),
        ];
        let (instructions, input) = responses_input(Some(&system), &messages);
        assert_eq!(instructions, "base");
        let input = input.as_array().expect("array");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        assert_eq!(input[1]["content"][0]["type"], "output_text");
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["call_id"], "c1");
        assert_eq!(input[3]["type"], "function_call_output");
        assert_eq!(input[3]["call_id"], "c1");
    }

    #[test]
    fn folds_system_messages_into_instructions() {
        let messages = vec![msg("system", vec![json!({"type":"text","text":"extra"})])];
        let (instructions, input) = responses_input(Some(&SystemPrompt::new("base")), &messages);
        assert_eq!(instructions, "base\n\nextra");
        assert!(input.as_array().unwrap().is_empty());
    }

    fn usage_of(usage: Value) -> crate::anthropic::types::Usage {
        let mut accumulator = ResponsesAccumulator::default();
        accumulator.take_usage(&usage);
        assert!(accumulator.usage_available);
        accumulator.usage
    }

    #[test]
    fn cached_input_tokens_are_split_out_of_the_prompt_total() {
        let usage = usage_of(json!({
            "input_tokens": 10_000,
            "output_tokens": 250,
            "input_tokens_details": { "cached_tokens": 8_000 },
        }));
        assert_eq!(usage.input_tokens, 2_000);
        assert_eq!(usage.cache_read_input_tokens, 8_000);
        assert_eq!(usage.output_tokens, 250);
    }

    #[test]
    fn nested_prompt_tokens_details_are_accepted_as_a_fallback() {
        let usage = usage_of(json!({
            "prompt_tokens": 10_000,
            "completion_tokens": 40,
            "prompt_tokens_details": { "cached_tokens": 7_000 },
        }));
        assert_eq!(usage.input_tokens, 3_000);
        assert_eq!(usage.cache_read_input_tokens, 7_000);
    }

    #[test]
    fn an_endpoint_that_reports_no_cache_detail_is_all_fresh_input() {
        let usage = usage_of(json!({ "input_tokens": 900, "output_tokens": 10 }));
        assert_eq!(usage.input_tokens, 900);
        assert_eq!(usage.cache_read_input_tokens, 0);
    }

    #[test]
    fn an_impossible_cached_figure_is_clamped_to_the_prompt() {
        let usage = usage_of(json!({
            "input_tokens": 100,
            "input_tokens_details": { "cached_tokens": 900 },
        }));
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.cache_read_input_tokens, 100);
    }

    fn push_event(event: Value) -> crate::error::Result<()> {
        let mut accumulator = ResponsesAccumulator::default();
        accumulator.push(&event, &mut |_| {})
    }

    #[test]
    fn a_null_error_on_a_completed_event_is_not_a_failure() {
        push_event(json!({
            "type": "response.completed",
            "error": null,
            "response": { "status": "completed" },
        }))
        .expect("ChatGPT puts error:null on successful completions");
    }

    #[test]
    fn a_chatgpt_usage_limit_reaches_the_desktop_as_their_words() {
        let error = push_event(json!({
            "type": "error",
            "error": {
                "type": "usage_limit_exceeded",
                "message": "You've hit your usage limit. Try again in 3 hours.",
            },
        }))
        .expect_err("usage limit is a stream failure");
        assert_eq!(
            error.provider_user_message(),
            Some("You've hit your usage limit. Try again in 3 hours.")
        );
    }

    #[test]
    fn a_failed_response_with_a_nested_error_is_shown() {
        let error = push_event(json!({
            "type": "response.failed",
            "response": {
                "error": {
                    "code": "server_error",
                    "message": "The model provider had an error.",
                },
            },
        }))
        .expect_err("response.failed is a stream failure");
        assert_eq!(
            error.provider_user_message(),
            Some("The model provider had an error.")
        );
    }

    #[test]
    fn an_empty_stream_error_stays_off_the_chat_bubble() {
        let error = push_event(json!({
            "type": "error",
            "error": { "type": "server_error" },
        }))
        .expect_err("empty message is still a failure");
        assert_eq!(error.provider_user_message(), None);
    }

    fn complete(events: &[Value]) -> Completion {
        let mut accumulator = ResponsesAccumulator::default();
        for event in events {
            accumulator
                .push(event, &mut |_| {})
                .expect("fixture events are valid");
        }
        accumulator.finish()
    }

    fn tool_uses_of(completion: &Completion) -> Vec<&Value> {
        completion
            .content
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
            .collect()
    }

    #[test]
    fn argument_deltas_keyed_by_item_id_join_the_call_id() {
        // ChatGPT streams args under `fc_…` and names the call `call_…` later.
        // Those used to become two tools; the leftover went back as call_id "".
        let completion = complete(&[
            json!({
                "type": "response.function_call_arguments.delta",
                "item_id": "fc_1",
                "delta": "{\"path\":\"a.rs\"}",
            }),
            json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "function_call",
                    "id": "fc_1",
                    "call_id": "call_1",
                    "name": "read_file",
                    "arguments": "{\"path\":\"a.rs\"}",
                },
            }),
        ]);
        let tools = tool_uses_of(&completion);
        assert_eq!(tools.len(), 1, "one streamed call is one tool");
        assert_eq!(tools[0]["id"], "call_1");
        assert_eq!(tools[0]["name"], "read_file");
        assert_eq!(tools[0]["input"]["path"], "a.rs");
    }

    #[test]
    fn an_empty_call_id_does_not_hide_the_item_id() {
        let completion = complete(&[json!({
            "type": "response.output_item.added",
            "item": {
                "type": "function_call",
                "id": "fc_1",
                "call_id": "",
                "name": "read_file",
                "arguments": "{}",
            },
        })]);
        let tools = tool_uses_of(&completion);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["id"], "fc_1");
    }

    #[test]
    fn four_lookups_do_not_leave_an_empty_call_id_on_the_next_request() {
        let mut events = Vec::new();
        for (item, call) in [
            ("fc_1", "call_z"),
            ("fc_2", "call_2"),
            ("fc_3", "call_3"),
            ("fc_4", "call_4"),
        ] {
            events.push(json!({
                "type": "response.function_call_arguments.delta",
                "item_id": item,
                "delta": "{}",
            }));
            events.push(json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "function_call",
                    "id": item,
                    "call_id": call,
                    "name": "read_file",
                    "arguments": "{}",
                },
            }));
        }
        let completion = complete(&events);
        let tools = tool_uses_of(&completion);
        assert_eq!(tools.len(), 4);
        assert!(tools
            .iter()
            .all(|tool| tool["id"].as_str().is_some_and(|id| !id.is_empty())));

        let messages = vec![
            msg("user", vec![json!({"type":"text","text":"look around"})]),
            Message::assistant(completion.content.clone()),
            msg(
                "user",
                tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "type": "tool_result",
                            "tool_use_id": tool["id"],
                            "content": "ok",
                        })
                    })
                    .collect(),
            ),
        ];
        let (_, input) = responses_input(None, &messages);
        let input = input.as_array().expect("array");
        assert_eq!(input.len(), 9, "user + 4 calls + 4 outputs");
        assert_eq!(input[5]["type"], "function_call_output");
        assert_eq!(
            input[5]["call_id"], "call_z",
            "preserve the order emitted by the model"
        );
        assert!(input.iter().all(|item| {
            item.get("call_id")
                .and_then(Value::as_str)
                .is_none_or(|id| !id.is_empty())
        }));
    }

    #[test]
    fn responses_input_drops_a_blank_call_id_already_in_history() {
        let messages = vec![
            msg("user", vec![json!({"type":"text","text":"hi"})]),
            msg(
                "assistant",
                vec![
                    json!({"type":"tool_use","id":"","name":"read_file","input":{}}),
                    json!({"type":"tool_use","id":"call_ok","name":"read_file","input":{}}),
                ],
            ),
            msg(
                "user",
                vec![
                    json!({"type":"tool_result","tool_use_id":"","content":"ghost"}),
                    json!({"type":"tool_result","tool_use_id":"call_ok","content":"ok"}),
                ],
            ),
        ];
        let (_, input) = responses_input(None, &messages);
        let input = input.as_array().expect("array");
        assert_eq!(input.len(), 3, "user + the real call + its output");
        assert_eq!(input[1]["call_id"], "call_ok");
        assert_eq!(input[2]["call_id"], "call_ok");
    }

    #[test]
    fn encrypted_reasoning_and_fc_item_ids_round_trip_across_tool_rounds() {
        let mut thinking_deltas = String::new();
        let mut started_tools = Vec::new();
        let mut accumulator = ResponsesAccumulator::default();
        for event in [
            json!({
                "type": "response.reasoning_summary_text.delta",
                "delta": "Inspecting workspace...",
            }),
            json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "reasoning",
                    "id": "rs_abc",
                    "summary": [{"type": "summary_text", "text": "Inspecting workspace..."}],
                    "encrypted_content": "enc_payload_123"
                }
            }),
            json!({
                "type": "response.output_item.added",
                "item": {
                    "type": "function_call",
                    "id": "fc_99",
                    "call_id": "call_99",
                    "name": "read_file",
                    "arguments": ""
                }
            }),
            json!({
                "type": "response.function_call_arguments.delta",
                "item_id": "fc_99",
                "delta": "{\"path\":\"README.md\"}"
            }),
            json!({
                "type": "response.completed",
                "response": { "status": "completed", "model": "gpt-6-luna" }
            }),
        ] {
            accumulator
                .push(&event, &mut |ev| {
                    if let StreamEvent::Thinking(s) = ev {
                        thinking_deltas.push_str(s);
                    }
                })
                .expect("valid stream event");
        }
        let completion = accumulator
            .finish_stream(None, &mut |ev| {
                if let StreamEvent::ToolCallStart { name, id } = ev {
                    started_tools.push((name.to_string(), id.to_string()));
                }
            })
            .unwrap();
        assert_eq!(thinking_deltas, "Inspecting workspace...");
        assert_eq!(
            started_tools,
            vec![("read_file".to_string(), "call_99".to_string())]
        );

        assert_eq!(completion.stop_reason.as_deref(), Some("tool_use"));

        let messages = vec![
            msg(
                "user",
                vec![
                    json!({"type": "text", "text": "check screenshot"}),
                    json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": "iVBORw0KGgo="
                        }
                    }),
                ],
            ),
            Message::assistant(completion.content),
            msg(
                "user",
                vec![json!({
                    "type": "tool_result",
                    "tool_use_id": "call_99",
                    "content": "# Zest"
                })],
            ),
        ];

        let (_, input) = responses_input(None, &messages);
        let input = input.as_array().expect("array");
        assert_eq!(input[0]["content"][1]["type"], "input_image");
        assert_eq!(
            input[0]["content"][1]["image_url"],
            "data:image/png;base64,iVBORw0KGgo="
        );
        assert_eq!(input[1]["type"], "reasoning");
        assert_eq!(input[1]["id"], "rs_abc");
        assert_eq!(input[1]["encrypted_content"], "enc_payload_123");
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["id"], "fc_99");
        assert_eq!(input[2]["call_id"], "call_99");
        assert_eq!(input[3]["type"], "function_call_output");
        assert_eq!(input[3]["call_id"], "call_99");
    }

    #[test]
    fn a_dropped_stream_never_dispatches_a_partial_tool() {
        fn reject_tool_start(event: StreamEvent<'_>) {
            assert!(!matches!(event, StreamEvent::ToolCallStart { .. }));
        }
        for text in ["", "Checking the project"] {
            let mut accumulator = ResponsesAccumulator::default();
            accumulator
                .push(
                    &json!({"type":"response.output_text.delta", "delta":text}),
                    &mut reject_tool_start,
                )
                .unwrap();
            let tool = json!({"type":"response.output_item.added","item":{
                "type":"function_call","id":"fc_1","call_id":"call_z","name":"read_file","arguments":""
            }});
            accumulator.push(&tool, &mut reject_tool_start).unwrap();
            let result = accumulator.finish_stream(None, &mut reject_tool_start);
            if text.is_empty() {
                assert!(matches!(result, Err(HarnessError::PrematureEof)));
            } else {
                let partial = result.unwrap();
                assert_eq!(partial.stop_reason.as_deref(), Some(STREAM_INTERRUPTED));
                assert_eq!(partial.content, vec![json!({"type":"text","text":text})]);
            }
        }
    }

    #[test]
    fn token_limit_only_dispatches_calls_confirmed_complete() {
        let partial = json!({"type":"function_call","id":"fc_1","call_id":"call_a",
            "name":"read_file","arguments":"","status":"incomplete"});
        let done = json!({"type":"function_call","id":"fc_2","call_id":"call_b",
            "name":"list_dir","arguments":"{}","status":"completed"});
        let result = complete(&[
            json!({"type":"response.output_item.added","item":partial}),
            json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"},
                "output":[partial,done]}}),
        ]);
        assert_eq!(result.stop_reason.as_deref(), Some("max_tokens"));
        let calls = tool_uses_of(&result);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["id"], "call_b");
    }

    #[test]
    fn malformed_tool_arguments_survive_as_string_for_repair() {
        let completion = complete(&[json!({
            "type": "response.output_item.done",
            "item": {
                "type": "function_call",
                "id": "fc_bad",
                "call_id": "call_bad",
                "name": "read_file",
                "arguments": "{\"path\":\"unclosed"
            }
        })]);
        let tools = tool_uses_of(&completion);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["input"], json!("{\"path\":\"unclosed"));
    }
}
