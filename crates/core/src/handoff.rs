//! Bounded, provider-neutral context handed to delegated workers.
//!
//! A worker needs enough history to understand why its subtask exists, but it
//! must not receive raw wire messages: those contain thinking signatures,
//! potentially huge file bodies, write payloads, and occasionally credentials.
//! This module projects wire history into a small JSON document made only from
//! model-readable text and deliberately selected tool metadata.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::anthropic::types::Message;

/// Hard ceiling for the pretty-printed JSON inserted into a worker prompt.
pub const MAX_HANDOFF_BYTES: usize = 24 * 1024;

const MAX_HANDOFF_MESSAGES: usize = 32;
const MAX_PROMPT_BYTES: usize = 3_000;
const MAX_MESSAGE_TEXT_BYTES: usize = 3_000;
const MAX_REASONING_BYTES: usize = 1_200;
const MAX_TOOL_RESULT_BYTES: usize = 800;
const MAX_TOOL_INPUT_BYTES: usize = 300;
const MAX_TOOL_ITEMS_PER_MESSAGE: usize = 8;

const SAFE_TOOL_INPUT_KEYS: &[&str] = &[
    "path",
    "pattern",
    "query",
    "glob",
    "kind",
    "name",
    "offset",
    "limit",
    "depth",
    "max_results",
    "timeout_ms",
];

#[derive(Debug, Clone, Serialize)]
pub struct ContextHandoff {
    version: u8,
    original_prompt: String,
    current_user_prompt: String,
    transcript: Vec<HandoffMessage>,
    #[serde(skip_serializing_if = "is_zero")]
    omitted_messages: usize,
}

#[derive(Debug, Clone, Serialize)]
struct HandoffMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_summary: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<HandoffToolCall>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_results: Vec<HandoffToolResult>,
    #[serde(skip_serializing_if = "is_zero")]
    attachments_omitted: usize,
    #[serde(skip_serializing_if = "is_zero")]
    tool_items_omitted: usize,
}

#[derive(Debug, Clone, Serialize)]
struct HandoffToolCall {
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    safe_input: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
struct HandoffToolResult {
    tool_call_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<String>,
    summary: String,
    #[serde(skip_serializing_if = "is_false")]
    is_error: bool,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl ContextHandoff {
    /// Build a sanitized projection. Empty histories have no useful handoff.
    pub fn from_messages(messages: &[Message]) -> Option<Self> {
        let prompts: Vec<String> = messages.iter().filter_map(user_prompt).collect();
        let original_prompt = prompts.first()?.clone();
        let current_user_prompt = prompts.last().cloned().unwrap_or_default();

        let mut tool_names = HashMap::new();
        let mut transcript: Vec<HandoffMessage> = messages
            .iter()
            .filter_map(|message| sanitize_message(message, &mut tool_names))
            .collect();
        let mut omitted_messages = 0;

        if transcript.len() > MAX_HANDOFF_MESSAGES {
            let remove = transcript.len() - MAX_HANDOFF_MESSAGES;
            transcript.drain(..remove);
            omitted_messages += remove;
        }

        let mut handoff = Self {
            version: 1,
            original_prompt,
            current_user_prompt,
            transcript,
            omitted_messages,
        };

        // Preserve the two prompt anchors and discard oldest detail first.
        while handoff.json().len() > MAX_HANDOFF_BYTES && !handoff.transcript.is_empty() {
            handoff.transcript.remove(0);
            handoff.omitted_messages += 1;
        }

        Some(handoff)
    }

    pub fn json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

fn user_prompt(message: &Message) -> Option<String> {
    if message.role != "user" {
        return None;
    }
    let mut text = message
        .content
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n\n");
    let attachments = message
        .content
        .iter()
        .filter(|block| {
            matches!(
                block.get("type").and_then(Value::as_str),
                Some("image") | Some("document")
            )
        })
        .count();
    if attachments > 0 {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&format!("[{attachments} user attachment(s) omitted]"));
    }
    let text = sanitize_text(&text, MAX_PROMPT_BYTES);
    (!text.trim().is_empty()).then_some(text)
}

fn sanitize_message(
    message: &Message,
    tool_names: &mut HashMap<String, String>,
) -> Option<HandoffMessage> {
    let mut text = Vec::new();
    let mut reasoning = Vec::new();
    let mut tool_calls = Vec::new();
    let mut tool_results = Vec::new();
    let mut attachments_omitted = 0;
    let mut tool_items_omitted = 0;

    for block in &message.content {
        match block.get("type").and_then(Value::as_str).unwrap_or("") {
            "text" => {
                if let Some(value) = block.get("text").and_then(Value::as_str) {
                    text.push(value);
                }
            }
            "thinking" => {
                if let Some(value) = block.get("thinking").and_then(Value::as_str) {
                    reasoning.push(value);
                }
            }
            "tool_use" => {
                let Some(id) = block.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let Some(name) = block.get("name").and_then(Value::as_str) else {
                    continue;
                };
                tool_names.insert(id.to_string(), name.to_string());
                if tool_calls.len() < MAX_TOOL_ITEMS_PER_MESSAGE {
                    tool_calls.push(HandoffToolCall {
                        id: id.to_string(),
                        name: name.to_string(),
                        safe_input: block.get("input").and_then(safe_tool_input),
                    });
                } else {
                    tool_items_omitted += 1;
                }
            }
            "tool_result" => {
                let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
                    continue;
                };
                if tool_results.len() < MAX_TOOL_ITEMS_PER_MESSAGE {
                    let summary = block.get("content").map(result_text).unwrap_or_default();
                    tool_results.push(HandoffToolResult {
                        tool_call_id: id.to_string(),
                        tool_name: tool_names.get(id).cloned(),
                        summary: sanitize_text(&summary, MAX_TOOL_RESULT_BYTES),
                        is_error: block
                            .get("is_error")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    });
                } else {
                    tool_items_omitted += 1;
                }
            }
            "image" | "document" => attachments_omitted += 1,
            _ => {}
        }
    }

    let text = optional_sanitized(text.join("\n\n"), MAX_MESSAGE_TEXT_BYTES);
    let reasoning_summary = optional_sanitized(reasoning.join("\n\n"), MAX_REASONING_BYTES);
    let empty = text.is_none()
        && reasoning_summary.is_none()
        && tool_calls.is_empty()
        && tool_results.is_empty()
        && attachments_omitted == 0;
    if empty {
        return None;
    }

    Some(HandoffMessage {
        role: message.role.clone(),
        text,
        reasoning_summary,
        tool_calls,
        tool_results,
        attachments_omitted,
        tool_items_omitted,
    })
}

fn safe_tool_input(input: &Value) -> Option<Value> {
    let object = input.as_object()?;
    let mut safe = Map::new();
    for key in SAFE_TOOL_INPUT_KEYS {
        let Some(value) = object.get(*key) else {
            continue;
        };
        let sanitized = match value {
            Value::String(value) => Value::String(sanitize_text(value, MAX_TOOL_INPUT_BYTES)),
            Value::Number(_) | Value::Bool(_) => value.clone(),
            _ => continue,
        };
        safe.insert((*key).to_string(), sanitized);
    }
    (!safe.is_empty()).then_some(Value::Object(safe))
}

fn result_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn optional_sanitized(value: String, max_bytes: usize) -> Option<String> {
    let value = sanitize_text(&value, max_bytes);
    (!value.trim().is_empty()).then_some(value)
}

fn sanitize_text(value: &str, max_bytes: usize) -> String {
    let clipped = clip_utf8(value, max_bytes);
    let without_data = data_url_re().replace_all(&clipped, "[attachment omitted]");
    let without_bearer = bearer_re().replace_all(&without_data, "Bearer [redacted]");
    let without_secrets = secret_assignment_re().replace_all(&without_bearer, "$1$2[redacted]");
    clip_utf8(&without_secrets, max_bytes)
}

/// Head-only clip that reserves room for its own ellipsis.
///
/// Stays a local function rather than moving to [`crate::bounded::ends_within`]:
/// a handoff wants the *beginning* of a redacted value, and pushing an `Option`
/// through a redaction path buys nothing when the fallback would be the
/// unclipped original.
fn clip_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let end = crate::bounded::floor_boundary(value, max_bytes.saturating_sub('…'.len_utf8()));
    format!("{}…", &value[..end])
}

fn data_url_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)data:[^,\s]{1,120},[a-z0-9+/=\r\n]+")
            .expect("context handoff data-url regex")
    })
}

fn bearer_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\bbearer\s+[a-z0-9._~+/=-]+").expect("context handoff bearer regex")
    })
}

fn secret_assignment_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?i)\b(api[_-]?key|access[_-]?token|refresh[_-]?token|token|password|secret|authorization|cookie|private[_-]?key)\b(\s*[:=]\s*)(?:"[^"]*"|'[^']*'|[^\s,;]+)"#,
        )
        .expect("context handoff credential regex")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preserves_conversation_reasoning_and_safe_tool_evidence() {
        let messages = vec![
            Message::user_text("Build the export flow. api_key=do-not-forward"),
            Message::assistant(vec![
                json!({
                    "type": "thinking",
                    "thinking": "Need to inspect the existing writer.",
                    "signature": "must-never-leave-wire-history"
                }),
                json!({ "type": "text", "text": "I will inspect it." }),
                json!({
                    "type": "tool_use",
                    "id": "tool-1",
                    "name": "read_file",
                    "input": {
                        "path": "src/export.rs",
                        "offset": 10,
                        "content": "write payload must be omitted"
                    }
                }),
            ]),
            Message::user_blocks(vec![json!({
                "type": "tool_result",
                "tool_use_id": "tool-1",
                "content": "writer uses atomic rename; token: also-secret",
                "is_error": false
            })]),
            Message::user_text("Keep the existing API."),
        ];

        let handoff = ContextHandoff::from_messages(&messages).unwrap();
        let json = handoff.json();
        assert!(json.contains("Build the export flow"));
        assert!(json.contains("Keep the existing API"));
        assert!(json.contains("Need to inspect the existing writer"));
        assert!(json.contains("src/export.rs"));
        assert!(json.contains("writer uses atomic rename"));
        assert!(json.contains("[redacted]"));
        assert!(!json.contains("do-not-forward"));
        assert!(!json.contains("also-secret"));
        assert!(!json.contains("must-never-leave-wire-history"));
        assert!(!json.contains("write payload must be omitted"));
    }

    #[test]
    fn omits_command_bodies_and_binary_attachments() {
        let messages = vec![
            Message::user_text("Run the checks"),
            Message::assistant(vec![json!({
                "type": "tool_use",
                "id": "tool-2",
                "name": "bash",
                "input": {
                    "command": "curl -H 'Authorization: Bearer very-secret'",
                    "timeout_ms": 120000
                }
            })]),
            Message::user_blocks(vec![json!({
                "type": "image",
                "source": { "type": "base64", "data": "enormous-payload" }
            })]),
        ];

        let json = ContextHandoff::from_messages(&messages).unwrap().json();
        assert!(json.contains("bash"));
        assert!(json.contains("timeout_ms"));
        assert!(json.contains("attachments_omitted"));
        assert!(!json.contains("curl"));
        assert!(!json.contains("very-secret"));
        assert!(!json.contains("enormous-payload"));
    }

    #[test]
    fn stays_within_budget_and_keeps_prompt_anchors() {
        let mut messages = vec![Message::user_text("original objective")];
        for index in 0..100 {
            messages.push(Message::assistant(vec![json!({
                "type": "text",
                "text": format!("assistant-{index}: {}", "x".repeat(4_000))
            })]));
            messages.push(Message::user_text(format!(
                "follow-up-{index}: {}",
                "y".repeat(4_000)
            )));
        }

        let handoff = ContextHandoff::from_messages(&messages).unwrap();
        let json = handoff.json();
        assert!(json.len() <= MAX_HANDOFF_BYTES, "{}", json.len());
        assert!(json.contains("original objective"));
        assert!(json.contains("follow-up-99"));
        assert!(handoff.omitted_messages > 0);
    }
}
