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
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": result.get("tool_use_id").and_then(Value::as_str).unwrap_or(""),
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
                    "call_id": call.get("id").and_then(Value::as_str).unwrap_or(""),
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
        if let Some(error) = event.get("error") {
            return Err(HarnessError::Stream {
                kind: error
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("provider_error")
                    .to_string(),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("ChatGPT Codex stream failed")
                    .to_string(),
            });
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
                    let key = event
                        .get("item_id")
                        .or_else(|| event.get("call_id"))
                        .and_then(Value::as_str)
                        .unwrap_or("0")
                        .to_string();
                    self.tools.entry(key).or_default().arguments.push_str(delta);
                }
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
                let id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let arguments = item.get("arguments").and_then(Value::as_str).unwrap_or("");
                let tool = self.tools.entry(id.clone()).or_default();
                if tool.id.is_empty() {
                    tool.id = id;
                }
                if tool.name.is_empty() {
                    tool.name = name;
                }
                if !arguments.is_empty() && tool.arguments.is_empty() {
                    tool.arguments.push_str(arguments);
                }
                if !tool.emitted && !tool.id.is_empty() && !tool.name.is_empty() {
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
        self.usage.input_tokens = usage
            .get("input_tokens")
            .or_else(|| usage.get("prompt_tokens"))
            .and_then(Value::as_u64)
            .map(|value| value as u32)
            .unwrap_or(self.usage.input_tokens);
        self.usage.output_tokens = usage
            .get("output_tokens")
            .or_else(|| usage.get("completion_tokens"))
            .and_then(Value::as_u64)
            .map(|value| value as u32)
            .unwrap_or(self.usage.output_tokens);
        self.usage_available = true;
    }

    fn finish(self) -> Completion {
        let mut content = Vec::new();
        if !self.text.is_empty() {
            content.push(json!({"type":"text", "text": self.text}));
        }
        let stop_reason = if self.tools.is_empty() {
            Some("end_turn".into())
        } else {
            Some("tool_use".into())
        };
        for tool in self.tools.into_values() {
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
}
