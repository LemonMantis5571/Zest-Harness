//! Wire types for the Messages API.
//!
//! Assistant content blocks are kept as `serde_json::Value`, not a typed enum.
//! That is deliberate: the API adds block types over time (`server_tool_use`,
//! `fallback`, ...), and thinking blocks carry a `signature` that must be echoed
//! back byte-for-byte on the next turn or the request is rejected. Round-tripping
//! the raw JSON is lossless by construction; a typed enum would silently drop
//! anything it didn't know about. Typed access is via `tool_uses()` below.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const API_BASE: &str = "https://api.anthropic.com";
pub const API_VERSION: &str = "2023-06-01";
pub const DEFAULT_MODEL: &str = "claude-opus-5";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: Vec<Value>,
}

impl Message {
    pub fn user_text(text: impl Into<String>) -> Self {
        Message {
            role: "user".into(),
            content: vec![json!({ "type": "text", "text": text.into() })],
        }
    }

    pub fn user_blocks(content: Vec<Value>) -> Self {
        Message {
            role: "user".into(),
            content,
        }
    }

    pub fn assistant(content: Vec<Value>) -> Self {
        Message {
            role: "assistant".into(),
            content,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// A cache breakpoint on the **last** tool covers the whole tool list, which
    /// is the largest fixed prefix of every request. Omitted entirely unless the
    /// provider says it understands caching.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<Value>,
}

/// `{"type": "ephemeral"}` — the five-minute breakpoint, for the moving end of
/// the conversation, which is rewritten every turn anyway.
pub fn ephemeral_cache_control() -> Value {
    json!({ "type": "ephemeral" })
}

/// `{"type": "ephemeral", "ttl": "1h"}` — for the parts of the prompt that are
/// fixed for a whole session.
///
/// The default five minutes is tuned for a request loop, not for a person. A
/// user who reads a reply for six minutes before typing the next one loses the
/// tools and system prefix and pays to rebuild it, which is the single largest
/// avoidable miss in a desktop chat. The hour costs 2x on write instead of
/// 1.25x and reads back at the same 0.1x, so it pays for itself on the first
/// re-read and every one after that is free money.
pub fn long_cache_control() -> Value {
    json!({ "type": "ephemeral", "ttl": "1h" })
}

/// A system prompt as cacheable text blocks.
///
/// The API accepts `system` as either a bare string or an array of blocks, and
/// only the array form can carry `cache_control`. Callers that do not cache keep
/// sending the string, so nothing changes on providers that would reject it.
///
/// `volatile` becomes a second, unmarked block after the breakpoint rather than
/// being folded into the first. Anything appended to the cached text would take
/// the whole block's key with it every time it changed; anything placed after
/// the breakpoint cannot.
pub fn cached_system_blocks(cacheable: &str, volatile: &str) -> Value {
    let mut blocks = vec![json!({
        "type": "text",
        "text": cacheable,
        "cache_control": long_cache_control(),
    })];
    if !volatile.is_empty() {
        blocks.push(json!({ "type": "text", "text": volatile }));
    }
    Value::Array(blocks)
}

#[derive(Debug, Clone, Serialize)]
pub struct Thinking {
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// `"summarized"` streams a readable summary. The API default is `"omitted"`,
    /// which still emits thinking blocks but with empty text — to a streaming UI
    /// that reads as a long stall before any output.
    pub display: &'static str,
}

impl Default for Thinking {
    fn default() -> Self {
        Thinking {
            kind: "adaptive",
            display: "summarized",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OutputConfig {
    pub effort: String,
}

/// Note what is absent: `temperature`, `top_p`, `top_k`. Those are rejected with
/// a 400 on Opus 5 — steering is done through the prompt, not sampling knobs.
#[derive(Debug, Clone, Serialize)]
pub struct Request {
    pub model: String,
    /// Caps thinking **and** response text together. Thinking is on by default
    /// on Opus 5, so a value tuned for text alone will truncate mid-answer.
    pub max_tokens: u32,
    pub stream: bool,
    /// Either a bare string or an array of text blocks — see
    /// [`cached_system_blocks`]. Kept as `Value` so the provider decides which
    /// shape the endpoint gets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<Value>,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDef>,
    /// Only ever `{"type": "none"}`, and only for maintenance turns that need
    /// the tool list present for the cached prefix but must not call anything.
    /// Changing it invalidates the message cache but *not* tools or system,
    /// which is exactly the trade being made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Thinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
}

impl Usage {
    /// Tokens the prompt actually occupied: fresh input plus both cache columns.
    ///
    /// `input_tokens` alone is the *uncached remainder*. On a well-cached turn
    /// that is a rounding error against the prompt it is a part of, so reading
    /// it as context occupancy understates the prompt by an order of magnitude.
    /// Every provider normalizes to this split — the OpenAI-compatible and Codex
    /// paths subtract their cached share out of the prompt they were handed for
    /// exactly this reason.
    ///
    /// Zero when the provider reported nothing, which is how a caller tells a
    /// silent endpoint from a measured one.
    pub fn prompt_tokens(&self) -> u64 {
        u64::from(self.input_tokens)
            .saturating_add(u64::from(self.cache_read_input_tokens))
            .saturating_add(u64::from(self.cache_creation_input_tokens))
    }
}

#[derive(Debug, Clone)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// Pull the client-side tool calls out of an assistant turn.
///
/// Only `tool_use` blocks — `server_tool_use` runs on Anthropic's side and needs
/// no result from us.
pub fn tool_uses(content: &[Value]) -> Vec<ToolUse> {
    content
        .iter()
        .filter_map(|block| {
            if block.get("type")?.as_str()? != "tool_use" {
                return None;
            }
            Some(ToolUse {
                id: block.get("id")?.as_str()?.to_string(),
                name: block.get("name")?.as_str()?.to_string(),
                input: block.get("input").cloned().unwrap_or_else(|| json!({})),
            })
        })
        .collect()
}

/// Concatenate a turn's text blocks, ignoring thinking and tool blocks.
pub fn text_of(content: &[Value]) -> String {
    content
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("")
}

/// A `tool_result` block. `tool_use_id` must match the `tool_use` it answers.
///
/// Every result for one assistant turn goes into a *single* user message —
/// splitting them across messages trains the model out of parallel tool calls.
pub fn tool_result(tool_use_id: &str, content: &str, is_error: bool) -> Value {
    json!({
        "type": "tool_result",
        "tool_use_id": tool_use_id,
        "content": content,
        "is_error": is_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_total_counts_both_cache_columns() {
        // A well-cached turn on a 128k window: the prompt is ~100k tokens, but
        // `input_tokens` on its own reads 1_200. Reading that as occupancy is
        // what reported a nearly-full window as almost empty.
        let usage = Usage {
            input_tokens: 1_200,
            output_tokens: 800,
            cache_read_input_tokens: 96_000,
            cache_creation_input_tokens: 2_800,
        };
        assert_eq!(usage.prompt_tokens(), 100_000);
    }

    #[test]
    fn a_silent_provider_reports_no_prompt() {
        assert_eq!(Usage::default().prompt_tokens(), 0);
    }

    #[test]
    fn output_tokens_are_not_part_of_the_prompt() {
        let usage = Usage {
            output_tokens: 4_096,
            ..Usage::default()
        };
        assert_eq!(usage.prompt_tokens(), 0);
    }
}
