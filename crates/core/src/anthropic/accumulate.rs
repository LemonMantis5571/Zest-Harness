//! Rebuilds an assistant turn from the SSE event stream.
//!
//! Split out from the HTTP client so it can be tested against a recorded event
//! stream instead of only over the network. This is the most failure-prone code
//! in the harness — partial-JSON reassembly and thinking-signature preservation
//! both fail in ways that look like a working response.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::types::Usage;
use crate::error::{HarnessError, Result};
use crate::provider::{Completion, RateLimitSnapshot, StreamEvent};

#[derive(Default)]
pub(crate) struct TurnAccumulator {
    /// Keyed by the stream's `index`. Ordering is explicit in the protocol and is
    /// never inferred from arrival order.
    blocks: BTreeMap<usize, Value>,
    /// `tool_use` inputs arrive as *fragments of a JSON string*, not JSON values.
    /// Accumulate per index and parse once the block closes.
    json_bufs: BTreeMap<usize, String>,
    stop_reason: Option<String>,
    usage: Usage,
    /// What `message_start` said actually served the turn.
    served_model: Option<String>,
    done: bool,
}

impl TurnAccumulator {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// True once `message_stop` has been seen.
    pub(crate) fn is_done(&self) -> bool {
        self.done
    }

    pub(crate) fn push(
        &mut self,
        ev: &Value,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    ) -> Result<()> {
        match ev.get("type").and_then(Value::as_str).unwrap_or("") {
            "message_start" => {
                if let Some(u) = ev.pointer("/message/usage") {
                    self.usage = serde_json::from_value(u.clone()).unwrap_or_default();
                }
                // The endpoint's own statement of which model ran. A gateway can
                // route a request anywhere, so this is the only place the answer
                // is available — the model's own guess does not count.
                self.served_model = ev
                    .pointer("/message/model")
                    .and_then(Value::as_str)
                    .filter(|m| !m.trim().is_empty())
                    .map(str::to_string);
            }

            "content_block_start" => {
                let idx = block_index(ev);
                let block = ev
                    .get("content_block")
                    .cloned()
                    .unwrap_or_else(|| json!({}));

                let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
                if matches!(kind, "tool_use" | "server_tool_use") {
                    self.json_bufs.insert(idx, String::new());
                    if let Some(name) = block.get("name").and_then(Value::as_str) {
                        let id = block.get("id").and_then(Value::as_str).unwrap_or("");
                        on_event(StreamEvent::ToolCallStart { name, id });
                    }
                }

                self.blocks.insert(idx, block);
            }

            "content_block_delta" => {
                let idx = block_index(ev);
                let Some(delta) = ev.get("delta") else {
                    return Ok(());
                };

                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => {
                        let t = delta.get("text").and_then(Value::as_str).unwrap_or("");
                        on_event(StreamEvent::Text(t));
                        append_str(entry(&mut self.blocks, idx, "text"), "text", t);
                    }
                    "thinking_delta" => {
                        let t = delta.get("thinking").and_then(Value::as_str).unwrap_or("");
                        on_event(StreamEvent::Thinking(t));
                        append_str(entry(&mut self.blocks, idx, "thinking"), "thinking", t);
                    }
                    "signature_delta" => {
                        // Integrity signature for the thinking block. Must survive
                        // round-tripping untouched or the next request is rejected.
                        if let Some(s) = delta.get("signature").and_then(Value::as_str) {
                            entry(&mut self.blocks, idx, "thinking")["signature"] = json!(s);
                        }
                    }
                    "input_json_delta" => {
                        if let Some(p) = delta.get("partial_json").and_then(Value::as_str) {
                            self.json_bufs.entry(idx).or_default().push_str(p);
                        }
                    }
                    // Unknown delta types are additive by policy — skip.
                    _ => {}
                }
            }

            "content_block_stop" => {
                let idx = block_index(ev);
                if let Some(buf) = self.json_bufs.remove(&idx) {
                    let parsed = if buf.trim().is_empty() {
                        json!({})
                    } else {
                        serde_json::from_str(&buf).map_err(|e| HarnessError::Stream {
                            kind: "malformed_tool_input".into(),
                            message: format!("{e} — accumulated: {buf}"),
                        })?
                    };
                    if let Some(block) = self.blocks.get_mut(&idx) {
                        block["input"] = parsed;
                    }
                }
            }

            "message_delta" => {
                if let Some(sr) = ev.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop_reason = Some(sr.to_string());
                }
                // Cumulative, not incremental — assign, don't add.
                if let Some(o) = ev.pointer("/usage/output_tokens").and_then(Value::as_u64) {
                    self.usage.output_tokens = o as u32;
                }
            }

            "message_stop" => self.done = true,

            "error" => {
                return Err(HarnessError::Stream {
                    kind: ev
                        .pointer("/error/type")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                    message: ev
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                });
            }

            // `ping`, and anything added to the protocol after this was written.
            _ => {}
        }

        Ok(())
    }

    pub(crate) fn finish(self, limits: Option<RateLimitSnapshot>) -> Completion {
        Completion {
            content: self.blocks.into_values().collect(),
            stop_reason: self.stop_reason,
            usage: self.usage,
            usage_available: true,
            limits,
            served_model: self.served_model,
            provider_session: None,
        }
    }
}

fn block_index(ev: &Value) -> usize {
    ev.get("index").and_then(Value::as_u64).unwrap_or(0) as usize
}

/// Get the block at `idx`, creating an empty one of `kind` if a delta somehow
/// arrives before its `content_block_start`. Shouldn't happen; don't panic if it does.
fn entry<'a>(blocks: &'a mut BTreeMap<usize, Value>, idx: usize, kind: &str) -> &'a mut Value {
    blocks.entry(idx).or_insert_with(|| {
        let mut map = serde_json::Map::new();
        map.insert("type".to_string(), Value::String(kind.to_string()));
        map.insert(kind.to_string(), Value::String(String::new()));
        Value::Object(map)
    })
}

fn append_str(block: &mut Value, field: &str, s: &str) {
    if s.is_empty() {
        return;
    }
    let mut cur = block
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    cur.push_str(s);
    block[field] = Value::String(cur);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::sse::SseParser;

    /// Drive a raw SSE transcript through the parser and accumulator, exactly as
    /// the HTTP client does.
    fn replay(sse: &str) -> (Completion, Vec<String>) {
        let mut parser = SseParser::default();
        let mut acc = TurnAccumulator::new();
        let mut seen: Vec<String> = Vec::new();

        {
            let mut sink = |e: StreamEvent<'_>| match e {
                StreamEvent::Text(t) => seen.push(format!("text:{t}")),
                StreamEvent::Thinking(t) => seen.push(format!("thinking:{t}")),
                StreamEvent::ProviderActivity { .. } => {}
                StreamEvent::ToolCallStart { name, .. } => seen.push(format!("tool:{name}")),
                StreamEvent::ToolCallUpdate { .. } => {}
                StreamEvent::ApprovalNeeded { tool_name, .. } => {
                    seen.push(format!("approval:{tool_name}"))
                }
                StreamEvent::ToolCallResult { name, .. } => {
                    seen.push(format!("tool_result:{name}"))
                }
                StreamEvent::QuestionNeeded { .. } => seen.push("question".into()),
                StreamEvent::ModelSubstituted { served, .. } => {
                    seen.push(format!("substituted:{served}"))
                }
                StreamEvent::ResumeHandle(_) => {}
            };
            for payload in parser.feed(sse.as_bytes()) {
                let ev: Value = serde_json::from_str(&payload).expect("valid event json");
                acc.push(&ev, &mut sink).expect("no stream error");
            }
        }

        (acc.finish(None), seen)
    }

    /// Verbatim from the published streaming reference, trimmed of repeated text
    /// deltas. This is the case the harness exists to get right.
    const TOOL_USE_STREAM: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_014p7gG3wDgGV9EUtLvnow3U","type":"message","role":"assistant","model":"claude-opus-5","stop_sequence":null,"usage":{"input_tokens":472,"output_tokens":2},"content":[],"stop_reason":null}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: ping
data: {"type": "ping"}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Okay"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":", let's check"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_01T1x1fJ34qAmk2tNTrN7Up6","name":"get_weather","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"location\":"}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":" \"San"}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":" Francisc"}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"o,"}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":" CA\"}"}}

event: content_block_stop
data: {"type":"content_block_stop","index":1}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":89}}

event: message_stop
data: {"type":"message_stop"}
"#;

    #[test]
    fn reassembles_a_tool_call_from_partial_json_fragments() {
        let (completion, seen) = replay(TOOL_USE_STREAM);

        assert_eq!(completion.content.len(), 2, "text block + tool_use block");
        assert_eq!(completion.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(completion.usage.output_tokens, 89);
        assert_eq!(completion.usage.input_tokens, 472);

        assert_eq!(completion.content[0]["type"], "text");
        assert_eq!(completion.content[0]["text"], "Okay, let's check");

        let tool = &completion.content[1];
        assert_eq!(tool["type"], "tool_use");
        assert_eq!(tool["id"], "toolu_01T1x1fJ34qAmk2tNTrN7Up6");
        assert_eq!(tool["name"], "get_weather");
        // The six fragments must land as one parsed object, not a string.
        assert_eq!(tool["input"], json!({"location": "San Francisco, CA"}));

        assert!(seen.contains(&"tool:get_weather".to_string()));
    }

    #[test]
    fn extracted_tool_call_survives_the_typed_view() {
        let (completion, _) = replay(TOOL_USE_STREAM);
        let calls = crate::anthropic::types::tool_uses(&completion.content);

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[0].input["location"], "San Francisco, CA");
    }

    #[test]
    fn preserves_the_thinking_signature() {
        let stream = r#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"1071 = 2 x 462 + 147"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"\n462 = 3 x 147 + 21"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"EqQBCgIYAhIM1gbcDa9GJwZA2b3h"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}

event: message_stop
data: {"type":"message_stop"}
"#;
        let (completion, seen) = replay(stream);

        let block = &completion.content[0];
        assert_eq!(block["type"], "thinking");
        assert_eq!(
            block["thinking"],
            "1071 = 2 x 462 + 147\n462 = 3 x 147 + 21"
        );
        // Dropping or altering this makes the *next* request fail, not this one.
        assert_eq!(block["signature"], "EqQBCgIYAhIM1gbcDa9GJwZA2b3h");
        assert_eq!(
            seen.len(),
            2,
            "one event per thinking delta, none for signature"
        );
    }

    #[test]
    fn ignores_event_and_delta_types_it_does_not_know() {
        let stream = r#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: some_future_event
data: {"type":"some_future_event","index":0,"payload":{"whatever":true}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"some_future_delta","value":"ignored"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"kept"}}

event: message_stop
data: {"type":"message_stop"}
"#;
        let (completion, _) = replay(stream);
        assert_eq!(completion.content[0]["text"], "kept");
    }

    #[test]
    fn surfaces_a_mid_stream_error_event() {
        let mut parser = SseParser::default();
        let mut acc = TurnAccumulator::new();
        let mut sink = |_: StreamEvent<'_>| {};

        let sse = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n";
        let payloads = parser.feed(sse.as_bytes());
        let ev: Value = serde_json::from_str(&payloads[0]).unwrap();

        let err = acc.push(&ev, &mut sink).unwrap_err();
        match err {
            HarnessError::Stream { kind, message } => {
                assert_eq!(kind, "overloaded_error");
                assert_eq!(message, "Overloaded");
            }
            other => panic!("expected a stream error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_tool_input_that_never_became_valid_json() {
        let stream = r#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_x","name":"t","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"a\": "}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}
"#;
        let mut parser = SseParser::default();
        let mut acc = TurnAccumulator::new();
        let mut sink = |_: StreamEvent<'_>| {};
        let mut result = Ok(());

        for payload in parser.feed(stream.as_bytes()) {
            let ev: Value = serde_json::from_str(&payload).unwrap();
            result = acc.push(&ev, &mut sink);
            if result.is_err() {
                break;
            }
        }

        // Truncated input must fail loudly rather than reach a tool as `{}`.
        assert!(matches!(
            result,
            Err(HarnessError::Stream { ref kind, .. }) if kind == "malformed_tool_input"
        ));
    }

    /// A gateway can route a request anywhere, so the model named in
    /// `message_start` is the only statement of what actually ran.
    #[test]
    fn records_the_model_message_start_reports() {
        let mut acc = TurnAccumulator::new();
        let mut sink = |_: StreamEvent<'_>| {};
        let ev: Value = serde_json::from_str(
            r#"{"type":"message_start","message":{"model":"claude-opus-5","usage":{"input_tokens":1,"output_tokens":0}}}"#,
        )
        .unwrap();
        acc.push(&ev, &mut sink).unwrap();
        assert_eq!(
            acc.finish(None).served_model.as_deref(),
            Some("claude-opus-5")
        );
    }

    #[test]
    fn a_stream_that_names_no_model_claims_none() {
        let mut acc = TurnAccumulator::new();
        let mut sink = |_: StreamEvent<'_>| {};
        let ev: Value =
            serde_json::from_str(r#"{"type":"message_start","message":{"usage":{}}}"#).unwrap();
        acc.push(&ev, &mut sink).unwrap();
        assert_eq!(acc.finish(None).served_model, None);
    }
}
