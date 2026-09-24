//! OpenAI Chat Completions-compatible provider.
//!
//! The rest of Zest keeps its durable conversation in the existing raw content
//! block shape. This module translates that shape at the HTTP boundary so the
//! agent loop and tool runner remain provider-neutral.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};

use super::{
    catalogue, Completion, EffortPolicy, ModelSpec, Provider, RateLimitSnapshot, StreamEvent,
    SystemPrompt, TurnRequest,
};
use crate::anthropic::sse::SseParser;
use crate::anthropic::types::{Message, ToolDef, Usage};
use crate::auth::AuthStatus;
use crate::cancel::{wait_cancel, CancelToken};
use crate::error::{HarnessError, Result};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const RESPONSE_HEADERS_TIMEOUT: Duration = Duration::from_secs(120);
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, Serialize)]
struct Request {
    model: String,
    messages: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Value>,
    stream: bool,
    stream_options: StreamOptions,
    max_tokens: u32,
}

#[derive(Debug, Clone, Serialize)]
struct StreamOptions {
    include_usage: bool,
}

pub struct OpenAiCompatibleProvider {
    id: String,
    client: OpenAiCompatibleClient,
    default_model: String,
    models: Vec<ModelSpec>,
    has_key: bool,
    requires_key: bool,
}

impl OpenAiCompatibleProvider {
    pub fn new(
        id: impl Into<String>,
        api_key: String,
        base_url: impl Into<String>,
        default_model: impl Into<String>,
    ) -> Result<Self> {
        let has_key = !api_key.trim().is_empty();
        Ok(Self {
            id: id.into(),
            client: OpenAiCompatibleClient::new(api_key, base_url)?,
            default_model: default_model.into(),
            models: Vec::new(),
            has_key,
            requires_key: has_key,
        })
    }

    pub fn with_models(mut self, models: Vec<ModelSpec>) -> Self {
        self.models = models;
        self
    }

    pub fn without_key_requirement(mut self) -> Self {
        self.requires_key = false;
        self
    }

    pub fn with_key_requirement(mut self) -> Self {
        self.requires_key = true;
        self
    }
}

#[async_trait]
impl Provider for OpenAiCompatibleProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn models(&self) -> Vec<ModelSpec> {
        if self.models.is_empty() {
            catalogue(&self.default_model, &[], &[], EffortPolicy::Unsupported)
        } else {
            self.models.clone()
        }
    }

    fn auth_status(&self) -> AuthStatus {
        if self.has_key || !self.requires_key {
            AuthStatus::Ready { account: None }
        } else {
            AuthStatus::Unconfigured
        }
    }

    async fn stream_turn(
        &self,
        req: &TurnRequest,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    ) -> Result<Completion> {
        let wire = Request {
            model: req.model.clone(),
            messages: convert_messages(
                req.system.as_ref().map(SystemPrompt::text).as_deref(),
                &req.messages,
            ),
            // This provider has no cached prefix to protect, so a maintenance
            // turn simply withholds the tools rather than sending them with a
            // `none` choice these endpoints do not all agree on.
            tools: if req.allow_tool_use {
                req.tools.iter().map(convert_tool).collect()
            } else {
                Vec::new()
            },
            stream: true,
            stream_options: StreamOptions {
                include_usage: true,
            },
            max_tokens: req.max_tokens,
        };
        self.client
            .stream_cancellable(&wire, on_event, req.cancel.as_ref())
            .await
    }
}

pub struct OpenAiCompatibleClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl OpenAiCompatibleClient {
    pub fn new(api_key: String, base_url: impl Into<String>) -> Result<Self> {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let parsed = reqwest::Url::parse(&base_url)
            .map_err(|e| HarnessError::Other(format!("invalid OpenAI-compatible base URL: {e}")))?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(HarnessError::Other(
                "OpenAI-compatible base URL must be an http(s) URL with a host".into(),
            ));
        }
        Ok(Self {
            http: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .build()?,
            api_key,
            base_url,
        })
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    async fn stream_cancellable(
        &self,
        req: &Request,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
        cancel: Option<&CancelToken>,
    ) -> Result<Completion> {
        let mut attempt = 0;
        let response = loop {
            attempt += 1;
            match self.send_once(req, cancel).await {
                Ok(response) => break response,
                Err((error, retry_after)) if attempt < MAX_ATTEMPTS && error.is_transient() => {
                    let delay = retry_after.unwrap_or_else(|| {
                        Duration::from_secs(1 << attempt.min(4).saturating_sub(1))
                    });
                    tokio::select! {
                        biased;
                        _ = wait_cancel(cancel) => return Err(HarnessError::Cancelled),
                        _ = tokio::time::sleep(delay) => {}
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

        let limits = rate_limits_from_headers(response.headers());
        let mut parser = SseParser::default();
        let mut body = response.bytes_stream();
        let mut accumulator = OpenAiAccumulator::default();
        let mut last_progress = tokio::time::Instant::now();

        loop {
            let idle_deadline = last_progress + STREAM_IDLE_TIMEOUT;
            tokio::select! {
                biased;
                _ = wait_cancel(cancel) => return Err(HarnessError::Cancelled),
                chunk = body.next() => match chunk {
                    Some(Ok(bytes)) => {
                        for payload in parser.feed(&bytes) {
                            if payload == "[DONE]" {
                                accumulator.done = true;
                                break;
                            }
                            let event: Value = serde_json::from_str(&payload)?;
                            if accumulator.push(&event, on_event)? {
                                last_progress = tokio::time::Instant::now();
                            }
                        }
                        if accumulator.done { break; }
                    }
                    Some(Err(error)) => return Err(error.into()),
                    None => break,
                },
                _ = tokio::time::sleep_until(idle_deadline) => {
                    return Err(HarnessError::StreamIdleTimeout);
                }
            }
        }

        if !accumulator.done {
            return Err(HarnessError::PrematureEof);
        }
        accumulator.finish(limits)
    }

    async fn send_once(
        &self,
        req: &Request,
        cancel: Option<&CancelToken>,
    ) -> std::result::Result<reqwest::Response, (HarnessError, Option<Duration>)> {
        let response = tokio::select! {
            biased;
            _ = wait_cancel(cancel) => return Err((HarnessError::Cancelled, None)),
            response = wait_for_response_headers(
                self.http.post(self.endpoint())
                    .header("authorization", format!("Bearer {}", self.api_key))
                    .header("content-type", "application/json")
                    .json(req)
                    .send(),
                RESPONSE_HEADERS_TIMEOUT,
            ) => match response {
                    Ok(response) => response,
                    Err(error) => return Err((error, None)),
                },
        };
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        Err((HarnessError::Api { status, body }, None))
    }
}

async fn wait_for_response_headers<F>(request: F, timeout: Duration) -> Result<reqwest::Response>
where
    F: std::future::Future<Output = std::result::Result<reqwest::Response, reqwest::Error>>,
{
    tokio::time::timeout(timeout, request)
        .await
        .map_err(|_| HarnessError::Other("OpenAI-compatible response headers timed out".into()))?
        .map_err(HarnessError::Http)
}

#[derive(Default)]
struct OpenAiAccumulator {
    text: String,
    tools: BTreeMap<usize, ToolAccum>,
    stop_reason: Option<String>,
    usage: Usage,
    usage_available: bool,
    /// The `model` every chunk carries — what actually served the request.
    served_model: Option<String>,
    done: bool,
}

#[derive(Default)]
struct ToolAccum {
    id: String,
    name: String,
    arguments: String,
    saw_arguments: bool,
    emitted: bool,
}

impl OpenAiAccumulator {
    fn push(
        &mut self,
        event: &Value,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    ) -> Result<bool> {
        if let Some(error) = event.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if message.is_empty() {
                return Err(HarnessError::Stream {
                    kind: "openai_stream".into(),
                    message: "OpenAI-compatible stream failed".into(),
                });
            }
            let kind = error
                .get("type")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or("error");
            return Err(HarnessError::from_provider_stream(kind, message));
        }
        // Every chunk repeats it; the first one that carries it is enough.
        if self.served_model.is_none() {
            self.served_model = event
                .get("model")
                .and_then(Value::as_str)
                .filter(|m| !m.trim().is_empty())
                .map(str::to_string);
        }
        if let Some(usage) = event.get("usage") {
            if !usage.is_null() {
                let prompt = usage
                    .get("prompt_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                // These endpoints cache the prompt prefix on their own and
                // report the hit here — but as a *subset* of `prompt_tokens`,
                // where the ledger (following Anthropic) keeps the two apart.
                // DeepSeek uses its own top-level `prompt_cache_hit_tokens`
                // field; the nested OpenAI field remains the fallback for
                // other compatible endpoints.
                //
                // Left unsplit, every cached token is filed as fresh input and
                // the measured hit rate for this provider is a flat zero no
                // matter how well its cache is working.
                let cached = usage
                    .get("prompt_cache_hit_tokens")
                    .or_else(|| {
                        usage
                            .get("prompt_tokens_details")
                            .and_then(|details| details.get("cached_tokens"))
                    })
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .min(prompt);
                self.usage.input_tokens = bounded_u32(prompt - cached);
                self.usage.cache_read_input_tokens = bounded_u32(cached);
                self.usage.output_tokens = bounded_u32(
                    usage
                        .get("completion_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                );
                self.usage_available = true;
            }
        }
        let Some(choice) = event
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
        else {
            return Ok(false);
        };
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.stop_reason = Some(
                match reason {
                    "tool_calls" => "tool_use",
                    "stop" => "end_turn",
                    other => other,
                }
                .into(),
            );
        }
        let Some(delta) = choice.get("delta") else {
            return Ok(false);
        };
        let mut progressed = false;
        if let Some(text) = delta.get("content").and_then(Value::as_str) {
            self.text.push_str(text);
            if !text.is_empty() {
                progressed = true;
                on_event(StreamEvent::Text(text));
            }
        }
        progressed |= ["reasoning", "reasoning_content", "analysis"]
            .iter()
            .filter_map(|field| delta.get(*field).and_then(Value::as_str))
            .any(|text| !text.is_empty());
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let tool = self.tools.entry(index).or_default();
                if let Some(id) = call.get("id").and_then(Value::as_str) {
                    tool.id.push_str(id);
                    progressed |= !id.is_empty();
                }
                if let Some(function) = call.get("function") {
                    if let Some(name) = function.get("name").and_then(Value::as_str) {
                        tool.name.push_str(name);
                        progressed |= !name.is_empty();
                    }
                    if let Some(raw_arguments) = function.get("arguments") {
                        let Some(arguments) = raw_arguments.as_str() else {
                            return Err(HarnessError::Stream {
                                kind: "malformed_tool_input".into(),
                                message:
                                    "OpenAI-compatible tool arguments must arrive as JSON text"
                                        .into(),
                            });
                        };
                        tool.saw_arguments = true;
                        tool.arguments.push_str(arguments);
                        progressed |= !arguments.is_empty();
                    }
                }
                if !tool.emitted && !tool.id.is_empty() && !tool.name.is_empty() {
                    on_event(StreamEvent::ToolCallStart {
                        name: &tool.name,
                        id: &tool.id,
                    });
                    tool.emitted = true;
                }
            }
        }
        Ok(progressed)
    }

    fn finish(self, limits: Option<RateLimitSnapshot>) -> Result<Completion> {
        let mut content = Vec::new();
        if !self.text.is_empty() {
            content.push(json!({"type":"text", "text": self.text}));
        }
        for tool in self.tools.into_values() {
            if tool.id.trim().is_empty() || tool.name.trim().is_empty() {
                return Err(HarnessError::Stream {
                    kind: "incomplete_tool_call".into(),
                    message: "OpenAI-compatible stream ended with an incomplete tool call".into(),
                });
            }
            if !tool.saw_arguments || tool.arguments.trim().is_empty() {
                return Err(HarnessError::Stream {
                    kind: "incomplete_tool_call".into(),
                    message: "OpenAI-compatible stream ended before tool arguments arrived".into(),
                });
            }
            let input: Value =
                serde_json::from_str(&tool.arguments).map_err(|error| HarnessError::Stream {
                    kind: "malformed_tool_input".into(),
                    message: format!("OpenAI-compatible tool arguments are invalid JSON: {error}"),
                })?;
            if !input.is_object() {
                return Err(HarnessError::Stream {
                    kind: "malformed_tool_input".into(),
                    message: "OpenAI-compatible tool arguments must be a JSON object".into(),
                });
            }
            content.push(json!({"type":"tool_use", "id":tool.id, "name":tool.name, "input":input}));
        }
        Ok(Completion {
            content,
            stop_reason: self.stop_reason,
            usage: self.usage,
            usage_available: self.usage_available,
            limits,
            served_model: self.served_model,
            provider_session: None,
        })
    }
}

/// Read the standard OpenAI-compatible short-window headers when an endpoint
/// sends them. These are real server values, not a projection of Zest's local
/// token ledger. Header names are kept vendor-neutral because gateways often
/// forward the same contract.
fn rate_limits_from_headers(headers: &reqwest::header::HeaderMap) -> Option<RateLimitSnapshot> {
    let text = |keys: &[&str]| {
        keys.iter().find_map(|key| {
            headers
                .get(*key)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        })
    };
    let number = |keys: &[&str]| text(keys).and_then(|value| value.parse::<u64>().ok());

    let snapshot = RateLimitSnapshot {
        requests_limit: number(&["x-ratelimit-limit-requests"]),
        requests_remaining: number(&["x-ratelimit-remaining-requests"]),
        requests_reset: text(&["x-ratelimit-reset-requests"]),
        tokens_limit: number(&["x-ratelimit-limit-tokens"]),
        tokens_remaining: number(&["x-ratelimit-remaining-tokens"]),
        input_tokens_remaining: None,
        output_tokens_remaining: None,
        tokens_reset: text(&["x-ratelimit-reset-tokens"]),
        retry_after_secs: number(&["retry-after"]),
        ..Default::default()
    };

    (!snapshot.is_empty()).then_some(snapshot)
}

fn convert_tool(tool: &ToolDef) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema,
        }
    })
}

fn convert_messages(system: Option<&str>, messages: &[Message]) -> Vec<Value> {
    let mut output = Vec::new();
    if let Some(system) = system.filter(|text| !text.is_empty()) {
        output.push(json!({"role": "system", "content": system}));
    }
    for message in messages {
        let mut text = String::new();
        let mut tool_calls = Vec::new();
        let mut tool_results = Vec::new();
        for block in &message.content {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => text.push_str(block.get("text").and_then(Value::as_str).unwrap_or("")),
                Some("tool_use") => tool_calls.push(json!({
                    "id": block.get("id").and_then(Value::as_str).unwrap_or(""),
                    "type": "function",
                    "function": {
                        "name": block.get("name").and_then(Value::as_str).unwrap_or(""),
                        "arguments": serde_json::to_string(block.get("input").unwrap_or(&json!({}))).unwrap_or_else(|_| "{}".to_string()),
                    }
                })),
                Some("tool_result") => tool_results.push(json!({
                    "role": "tool",
                    "tool_call_id": block.get("tool_use_id").and_then(Value::as_str).unwrap_or(""),
                    "content": block.get("content").and_then(Value::as_str).unwrap_or(""),
                })),
                _ => {}
            }
        }
        if !tool_results.is_empty() {
            if !text.is_empty() {
                output.push(json!({"role":"user", "content":text}));
            }
            output.extend(tool_results);
            continue;
        }
        let mut message_json = json!({"role": message.role, "content": if text.is_empty() { Value::Null } else { Value::String(text) }});
        if !tool_calls.is_empty() {
            message_json["tool_calls"] = Value::Array(tool_calls);
        }
        output.push(message_json);
    }
    output
}

/// Saturate rather than wrap. A provider that reports a nonsense token count
/// should show as an implausibly large one, not as a small one.
fn bounded_u32(value: u64) -> u32 {
    value.min(u64::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn request_waiting_for_response_headers_has_a_deadline() {
        let request =
            std::future::pending::<std::result::Result<reqwest::Response, reqwest::Error>>();
        let error = wait_for_response_headers(request, Duration::from_millis(1))
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            HarnessError::Other(message) if message.contains("response headers timed out")
        ));
    }

    fn usage_of(usage: Value) -> Usage {
        let mut accumulator = OpenAiAccumulator::default();
        let mut sink = |_: StreamEvent<'_>| {};
        accumulator
            .push(&json!({ "usage": usage, "choices": [] }), &mut sink)
            .unwrap();
        assert!(accumulator.usage_available);
        accumulator.usage
    }

    #[test]
    fn cached_prompt_tokens_are_split_out_of_the_prompt_total() {
        // `cached_tokens` is a subset of `prompt_tokens` on the wire; counting
        // it in both columns would double the prompt and report a zero hit
        // rate on a provider whose cache is working perfectly well.
        let usage = usage_of(json!({
            "prompt_tokens": 10_000,
            "completion_tokens": 250,
            "prompt_tokens_details": { "cached_tokens": 8_000 },
        }));
        assert_eq!(usage.input_tokens, 2_000);
        assert_eq!(usage.cache_read_input_tokens, 8_000);
        assert_eq!(usage.output_tokens, 250);
    }

    #[test]
    fn deepseek_cache_hit_tokens_are_split_out_of_the_prompt_total() {
        // DeepSeek reports cache usage as top-level fields rather than the
        // nested `prompt_tokens_details.cached_tokens` shape used by OpenAI.
        let usage = usage_of(json!({
            "prompt_tokens": 10_000,
            "prompt_cache_hit_tokens": 8_000,
            "prompt_cache_miss_tokens": 2_000,
            "completion_tokens": 250,
        }));
        assert_eq!(usage.input_tokens, 2_000);
        assert_eq!(usage.cache_read_input_tokens, 8_000);
        assert_eq!(usage.output_tokens, 250);
    }

    #[test]
    fn an_endpoint_that_reports_no_cache_detail_is_all_fresh_input() {
        let usage = usage_of(json!({ "prompt_tokens": 900, "completion_tokens": 10 }));
        assert_eq!(usage.input_tokens, 900);
        assert_eq!(usage.cache_read_input_tokens, 0);
    }

    /// More cached than prompt is not physical, but it must not underflow into
    /// four billion fresh input tokens either.
    #[test]
    fn an_impossible_cached_figure_is_clamped_to_the_prompt() {
        let usage = usage_of(json!({
            "prompt_tokens": 100,
            "prompt_tokens_details": { "cached_tokens": 900 },
        }));
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.cache_read_input_tokens, 100);
    }

    #[test]
    fn reads_standard_rate_limit_headers() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "x-ratelimit-limit-requests",
            reqwest::header::HeaderValue::from_static("500"),
        );
        headers.insert(
            "x-ratelimit-remaining-requests",
            reqwest::header::HeaderValue::from_static("499"),
        );
        headers.insert(
            "x-ratelimit-limit-tokens",
            reqwest::header::HeaderValue::from_static("100000"),
        );
        headers.insert(
            "x-ratelimit-remaining-tokens",
            reqwest::header::HeaderValue::from_static("99000"),
        );

        let limits = rate_limits_from_headers(&headers).expect("limits");
        assert_eq!(limits.requests_limit, Some(500));
        assert_eq!(limits.requests_remaining, Some(499));
        assert_eq!(limits.tokens_limit, Some(100_000));
        assert_eq!(limits.tokens_remaining, Some(99_000));
    }

    #[test]
    fn converts_parallel_tools_and_results() {
        let messages = vec![
            Message::assistant(vec![
                json!({"type":"tool_use","id":"a","name":"read","input":{"path":"a"}}),
                json!({"type":"tool_use","id":"b","name":"read","input":{"path":"b"}}),
            ]),
            Message::user_blocks(vec![
                json!({"type":"tool_result","tool_use_id":"a","content":"A"}),
                json!({"type":"tool_result","tool_use_id":"b","content":"B"}),
            ]),
        ];
        let converted = convert_messages(Some("system"), &messages);
        assert_eq!(converted[0]["role"], "system");
        assert_eq!(converted[1]["tool_calls"].as_array().unwrap().len(), 2);
        assert_eq!(converted[2]["role"], "tool");
        assert_eq!(converted[3]["tool_call_id"], "b");
    }

    #[test]
    fn accumulates_tool_call_fragments_and_usage() {
        let mut accumulator = OpenAiAccumulator::default();
        let mut events = Vec::new();
        let mut sink = |event: StreamEvent<'_>| {
            if let StreamEvent::ToolCallStart { name, .. } = event {
                events.push(name.to_string());
            }
        };
        accumulator.push(&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_","function":{"name":"read","arguments":"{\"path\":\""}}]}}]}), &mut sink).unwrap();
        accumulator.push(&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"README.md\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":4,"completion_tokens":2}}), &mut sink).unwrap();
        let completion = accumulator.finish(None).unwrap();
        assert_eq!(events, vec!["read"]);
        assert_eq!(completion.stop_reason.as_deref(), Some("tool_use"));
        assert!(completion.usage_available);
        assert_eq!(completion.content[0]["input"]["path"], "README.md");
    }

    /// The endpoint's own statement of which model ran.
    ///
    /// Zest used to drop this, so it could only ever report the model it had
    /// *asked* for — which is not evidence. Asking the model itself is worse:
    /// it guesses.
    #[test]
    fn records_the_model_the_endpoint_says_it_used() {
        let mut accumulator = OpenAiAccumulator::default();
        let mut sink = |_: StreamEvent<'_>| {};
        accumulator
            .push(
                &json!({"model":"deepseek-v4-flash","choices":[{"delta":{"content":"hi"}}]}),
                &mut sink,
            )
            .unwrap();
        // Later chunks repeat it; the first answer stands.
        accumulator
            .push(
                &json!({"model":"deepseek-v4-flash","choices":[{"delta":{},"finish_reason":"stop"}]}),
                &mut sink,
            )
            .unwrap();
        assert_eq!(
            accumulator.finish(None).unwrap().served_model.as_deref(),
            Some("deepseek-v4-flash")
        );
    }

    #[test]
    fn a_silent_endpoint_reports_nothing_rather_than_agreement() {
        // No `model` field must not be read as "served what you asked for".
        let mut accumulator = OpenAiAccumulator::default();
        let mut sink = |_: StreamEvent<'_>| {};
        accumulator
            .push(&json!({"choices":[{"delta":{"content":"hi"}}]}), &mut sink)
            .unwrap();
        assert_eq!(accumulator.finish(None).unwrap().served_model, None);

        // An empty string is silence too, not a model named "".
        let mut blank = OpenAiAccumulator::default();
        blank
            .push(&json!({"model":"  ","choices":[{"delta":{}}]}), &mut sink)
            .unwrap();
        assert_eq!(blank.finish(None).unwrap().served_model, None);
    }

    #[test]
    fn malformed_tool_arguments_fail_instead_of_becoming_an_empty_object() {
        let mut accumulator = OpenAiAccumulator::default();
        let mut sink = |_: StreamEvent<'_>| {};
        accumulator
            .push(
                &json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":"{"}}]}}]}),
                &mut sink,
            )
            .unwrap();

        assert!(matches!(
            accumulator.finish(None),
            Err(HarnessError::Stream { kind, .. }) if kind == "malformed_tool_input"
        ));
    }

    #[test]
    fn null_tool_arguments_are_rejected() {
        let mut accumulator = OpenAiAccumulator::default();
        let mut sink = |_: StreamEvent<'_>| {};

        let result = accumulator.push(
            &json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":null}}]}}]}),
            &mut sink,
        );

        assert!(matches!(
            result,
            Err(HarnessError::Stream { kind, .. }) if kind == "malformed_tool_input"
        ));
    }

    #[test]
    fn keepalive_events_do_not_count_as_stream_progress() {
        let mut accumulator = OpenAiAccumulator::default();
        let mut sink = |_: StreamEvent<'_>| {};

        assert!(!accumulator
            .push(&json!({"choices": []}), &mut sink)
            .unwrap());
        assert!(!accumulator
            .push(&json!({"choices": [{"delta": {"content": ""}}]}), &mut sink)
            .unwrap());
        assert!(accumulator
            .push(
                &json!({"choices": [{"delta": {"content": "hello"}}]}),
                &mut sink
            )
            .unwrap());
    }

    #[test]
    fn empty_or_missing_tool_arguments_are_incomplete() {
        for arguments in [None, Some("")] {
            let mut accumulator = OpenAiAccumulator::default();
            let mut sink = |_: StreamEvent<'_>| {};
            let mut function = serde_json::Map::new();
            function.insert("name".into(), json!("read_file"));
            if let Some(arguments) = arguments {
                function.insert("arguments".into(), json!(arguments));
            }
            accumulator
                .push(
                    &json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":function}]}}]}),
                    &mut sink,
                )
                .unwrap();

            assert!(matches!(
                accumulator.finish(None),
                Err(HarnessError::Stream { kind, .. }) if kind == "incomplete_tool_call"
            ));
        }
    }
}
