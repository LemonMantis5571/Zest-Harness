//! ChatGPT Codex parent that owns Zest's agent loop.
//!
//! Talks to the unpublished Codex Responses backend. Identify as Zest; do not
//! impersonate the official CLI.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};

use super::{
    catalogue, Completion, EffortPolicy, ModelSpec, Provider, StreamEvent, SystemPrompt,
    TurnRequest, CODEX_KNOWN_MODELS,
};
use crate::anthropic::sse::SseParser;
use crate::anthropic::types::{Message, ToolDef, Usage};
use crate::auth::AuthStatus;
use crate::cancel::{wait_cancel, CancelToken};
use crate::codex_oauth::{refresh_and_store, CodexOAuthSession, BACKEND_URL, ORIGINATOR};
use crate::error::{HarnessError, Result};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_ATTEMPTS: u32 = 3;

pub struct CodexOAuthProvider {
    id: String,
    account: String,
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
        Self {
            id: id.into(),
            account: account.into(),
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

        let body = request_body(
            &req.model,
            req.effort.as_deref(),
            req.system.as_ref(),
            &req.messages,
            if req.allow_tool_use {
                req.tools.as_slice()
            } else {
                &[]
            },
        );
        stream_responses(&session, &body, on_event, req.cancel.as_ref()).await
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
        let mut tool_calls = Vec::new();
        let mut tool_results = Vec::new();
        for block in &message.content {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    text.push_str(block.get("text").and_then(Value::as_str).unwrap_or(""))
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
                    "output": result.get("content").and_then(Value::as_str).unwrap_or(""),
                }));
            }
            continue;
        }
        if message.role == "assistant" && !tool_calls.is_empty() {
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
                input.push(json!({
                    "type": "function_call",
                    "call_id": call_id,
                    "name": call.get("name").and_then(Value::as_str).unwrap_or(""),
                    "arguments": arguments,
                }));
            }
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

fn message_item(role: &str, part_type: &str, text: &str) -> Value {
    json!({
        "type": "message",
        "role": role,
        "content": [{ "type": part_type, "text": text }],
    })
}

async fn stream_responses(
    session: &CodexOAuthSession,
    body: &Value,
    on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    cancel: Option<&CancelToken>,
) -> Result<Completion> {
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()?;
    let mut attempt = 0;
    let response = loop {
        attempt += 1;
        match send_once(&client, session, body, cancel).await {
            Ok(response) => break response,
            Err((error, _)) if attempt < MAX_ATTEMPTS && error.is_transient() => {
                tokio::select! {
                    biased;
                    _ = wait_cancel(cancel) => return Err(HarnessError::Cancelled),
                    _ = tokio::time::sleep(Duration::from_secs(1 << attempt.min(4).saturating_sub(1))) => {}
                }
            }
            Err((error, _)) => {
                return Err(if attempt > 1 {
                    HarnessError::Exhausted {
                        attempts: attempt,
                        source: Box::new(error),
                    }
                } else {
                    error
                })
            }
        }
    };

    let mut parser = SseParser::default();
    let mut body_stream = response.bytes_stream();
    let mut accumulator = ResponsesAccumulator::default();
    loop {
        tokio::select! {
            biased;
            _ = wait_cancel(cancel) => return Err(HarnessError::Cancelled),
            chunk = body_stream.next() => match chunk {
                Some(Ok(bytes)) => {
                    for payload in parser.feed(&bytes) {
                        if payload == "[DONE]" {
                            accumulator.done = true;
                            break;
                        }
                        let event: Value = serde_json::from_str(&payload)?;
                        accumulator.push(&event, on_event)?;
                    }
                    if accumulator.done { break; }
                }
                Some(Err(error)) => return Err(error.into()),
                None => break,
            },
            _ = tokio::time::sleep(STREAM_IDLE_TIMEOUT) => {
                return Err(HarnessError::StreamIdleTimeout);
            }
        }
    }
    if !accumulator.done && accumulator.text.is_empty() && accumulator.tools.is_empty() {
        return Err(HarnessError::PrematureEof);
    }
    Ok(accumulator.finish())
}

async fn send_once(
    client: &reqwest::Client,
    session: &CodexOAuthSession,
    body: &Value,
    cancel: Option<&CancelToken>,
) -> std::result::Result<reqwest::Response, (HarnessError, Option<Duration>)> {
    let response = tokio::select! {
        biased;
        _ = wait_cancel(cancel) => return Err((HarnessError::Cancelled, None)),
        response = client
            .post(format!("{BACKEND_URL}/responses"))
            .header("authorization", format!("Bearer {}", session.access_token))
            .header("ChatGPT-Account-ID", &session.account_id)
            .header("originator", ORIGINATOR)
            .header("accept", "text/event-stream")
            .header("content-type", "application/json")
            .json(body)
            .send() => match response {
                Ok(response) => response,
                Err(error) => return Err((HarnessError::Http(error), None)),
            },
    };
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    Err((HarnessError::Api { status, body }, None))
}

#[derive(Default)]
struct ResponsesAccumulator {
    text: String,
    tools: BTreeMap<String, ToolAccum>,
    usage: Usage,
    usage_available: bool,
    served_model: Option<String>,
    done: bool,
}

#[derive(Default)]
struct ToolAccum {
    id: String,
    item_id: String,
    name: String,
    arguments: String,
    emitted: bool,
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
            "response.output_text.delta" | "response.reasoning_text.delta" => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    if kind == "response.output_text.delta" {
                        self.text.push_str(delta);
                        on_event(StreamEvent::Text(delta));
                    } else {
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
            "response.failed" | "response.incomplete" => {
                if let Some(error) = event.pointer("/response/error") {
                    return Err(chatgpt_stream_error(error));
                }
                if kind == "response.failed" {
                    return Err(HarnessError::from_provider_stream(
                        "error",
                        "ChatGPT stopped the reply before it finished.",
                    ));
                }
                self.done = true;
            }
            "response.completed" | "response.done" => {
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
                if !arguments.is_empty() && tool.arguments.is_empty() {
                    tool.arguments.push_str(arguments);
                }
                // Wait for ChatGPT's call_id so the UI row and the next request
                // agree. An item id is only a join key.
                if !tool.emitted && call_id.is_some() && !tool.name.is_empty() {
                    on_event(StreamEvent::ToolCallStart {
                        name: &tool.name,
                        id: &tool.id,
                    });
                    tool.emitted = true;
                }
            }
            Some("message") => {
                if let Some(text) = item
                    .get("content")
                    .and_then(Value::as_array)
                    .and_then(|parts| parts.first())
                    .and_then(|part| part.get("text"))
                    .and_then(Value::as_str)
                {
                    if self.text.is_empty() {
                        self.text.push_str(text);
                        on_event(StreamEvent::Text(text));
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
        if let (Some(call), Some(item)) = (call_id, item_id) {
            if call != item {
                if let Some(orphan) = self.tools.remove(item) {
                    merge_tool(self.tools.entry(call.to_string()).or_default(), orphan);
                }
            }
        }
        let key = match (call_id, item_id) {
            (Some(call), _) => call.to_string(),
            (None, Some(item)) => self
                .tools
                .iter()
                .find_map(|(key, tool)| {
                    (tool.item_id == item || tool.id == item || key == item).then(|| key.clone())
                })
                .unwrap_or_else(|| item.to_string()),
            (None, None) => return None,
        };
        let tool = self.tools.entry(key).or_default();
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

    fn finish(self) -> Completion {
        let mut content = Vec::new();
        if !self.text.is_empty() {
            content.push(json!({"type":"text", "text": self.text}));
        }
        let tools: Vec<ToolAccum> = self
            .tools
            .into_values()
            .filter(|tool| !tool.id.is_empty() && !tool.name.is_empty())
            .collect();
        let stop_reason = if tools.is_empty() {
            Some("end_turn".into())
        } else {
            Some("tool_use".into())
        };
        for tool in tools {
            let input = serde_json::from_str(&tool.arguments).unwrap_or_else(|_| json!({}));
            content.push(json!({
                "type": "tool_use",
                "id": tool.id,
                "name": tool.name,
                "input": input
            }));
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
    if dest.arguments.is_empty() {
        dest.arguments = src.arguments;
    } else if src.arguments.len() > dest.arguments.len() {
        dest.arguments = src.arguments;
    }
    dest.emitted |= src.emitted;
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
            ("fc_1", "call_1"),
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
        assert_eq!(input[5]["call_id"], "call_1");
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
}
