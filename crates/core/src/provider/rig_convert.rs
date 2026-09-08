//! Translation between Zest's wire history and Rig's typed message model.
//!
//! Zest keeps conversation history in [`crate::anthropic::types::Message`],
//! which is Anthropic-*shaped* but is really this crate's internal normalized
//! form: every provider translates out of it. Rig models the same conversation
//! as a typed enum. This module is the seam.
//!
//! # Why the tool-call identifiers get two fields
//!
//! Rig splits a call identifier in two. [`ToolCallId`] is rig's own correlation
//! handle, minted when a provider issued none, and [`ProviderCallId`] is what
//! the provider actually sent. Only the second may travel back upstream, and
//! `ToolResult::wire_call_id` prefers it. Zest's ids always come from a real
//! provider round trip, so both are set from the same string: rig correlates on
//! its handle and the wire still sees the id the provider minted. Setting only
//! the rig handle would put a synthetic id on the wire and the API would reject
//! the `tool_result` as answering a call that does not exist.
//!
//! # What this module refuses to do
//!
//! It does not invent content. A block whose shape does not round-trip is
//! reported through [`ConvertError`] rather than dropped, because a silently
//! dropped block is the failure this seam exists to prevent: on Anthropic,
//! altering the middle of a history invalidates every later thinking block, and
//! the resulting 400 arrives turns later with nothing pointing back here.

use rig_core::completion::message::{
    AssistantContent, Message as RigMessage, ProviderCallId, Reasoning, ReasoningContent, Text,
    ToolCall, ToolCallId, ToolFunction, ToolResult, ToolResultContent, UserContent,
};
use serde_json::Value;

use crate::anthropic::types::{Message, ToolDef};
use crate::provider::SystemPrompt;

/// A block that cannot be represented in Rig's model without losing something.
///
/// Carries the block type so a failure names what was unrepresentable rather
/// than only that something was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertError {
    pub block_type: String,
    pub reason: String,
}

impl ConvertError {
    fn new(block_type: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            block_type: block_type.into(),
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for ConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cannot convert `{}` block: {}", self.block_type, self.reason)
    }
}

impl std::error::Error for ConvertError {}

/// Split a Zest history into Rig's preamble plus chat history.
///
/// System content is returned separately rather than as a leading
/// [`RigMessage::System`]: the providers Zest targets hoist system content to a
/// single instruction field anyway, and keeping it out of `chat_history` means
/// the caller decides where it lands.
///
/// Messages that convert to zero blocks are dropped. Rig rejects an empty
/// content vector at the request boundary, and an empty message is not
/// information a provider can act on. Every drop is a message that carried
/// nothing convertible, never a message that carried something unrepresentable:
/// that case is an error instead.
pub fn to_rig_history(
    system: Option<&SystemPrompt>,
    messages: &[Message],
) -> Result<(Option<String>, Vec<RigMessage>), ConvertError> {
    let mut instructions = system.map(SystemPrompt::text).unwrap_or_default();
    let mut history = Vec::with_capacity(messages.len());

    for message in messages {
        match message.role.as_str() {
            "system" => {
                let text = text_of(&message.content);
                if !text.is_empty() {
                    if !instructions.is_empty() {
                        instructions.push_str("\n\n");
                    }
                    instructions.push_str(&text);
                }
            }
            "assistant" => {
                let content = assistant_content(&message.content)?;
                if !content.is_empty() {
                    history.push(RigMessage::Assistant { id: None, content });
                }
            }
            // Anything not assistant or system is a user turn. Zest only ever
            // writes "user" here, and treating an unknown role as user is the
            // safe direction: it keeps the content in the conversation instead
            // of silently deleting a turn.
            _ => {
                let content = user_content(&message.content)?;
                if !content.is_empty() {
                    history.push(RigMessage::User { content });
                }
            }
        }
    }

    let instructions = (!instructions.is_empty()).then_some(instructions);
    Ok((instructions, history))
}

fn assistant_content(blocks: &[Value]) -> Result<Vec<AssistantContent>, ConvertError> {
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        let kind = block_type(block);
        match kind {
            "text" => {
                let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                // An empty text block carries nothing and rig rejects empty
                // content, so it is skipped rather than emitted.
                if !text.is_empty() {
                    out.push(AssistantContent::Text(Text::new(text)));
                }
            }
            "thinking" => out.push(AssistantContent::Reasoning(reasoning(block))),
            "redacted_thinking" => {
                let data = block.get("data").and_then(Value::as_str).unwrap_or_default();
                out.push(AssistantContent::Reasoning(Reasoning {
                    id: None,
                    content: vec![ReasoningContent::Redacted { data: data.into() }],
                }));
            }
            // Written by `from_rig_content` for an OpenAI Responses encrypted
            // reasoning item. It exists only so that payload survives a round
            // trip through Zest's history, so it has to be read back here.
            "reasoning_encrypted" => {
                let data = block.get("data").and_then(Value::as_str).unwrap_or_default();
                out.push(AssistantContent::Reasoning(Reasoning {
                    id: None,
                    content: vec![ReasoningContent::Encrypted(data.into())],
                }));
            }
            "tool_use" => out.push(AssistantContent::ToolCall(tool_call(block)?)),
            other => {
                return Err(ConvertError::new(
                    other,
                    "rig's AssistantContent has no variant for this block and no \
                     passthrough, so converting would drop it",
                ))
            }
        }
    }
    Ok(out)
}

fn user_content(blocks: &[Value]) -> Result<Vec<UserContent>, ConvertError> {
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        match block_type(block) {
            "text" => {
                let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                if !text.is_empty() {
                    out.push(UserContent::Text(Text::new(text)));
                }
            }
            "tool_result" => out.push(UserContent::ToolResult(tool_result(block)?)),
            other => {
                return Err(ConvertError::new(
                    other,
                    "unsupported user block; images and documents need an explicit \
                     mapping before this provider can carry them",
                ))
            }
        }
    }
    Ok(out)
}

/// A thinking block, with its signature preserved.
///
/// The signature is what the provider validates on replay, so it rides in
/// [`ReasoningContent::Text`] rather than being reduced to prose. A block whose
/// `thinking` text is empty is still emitted: on models where `display` is
/// `"omitted"` the text is empty by design and only the signature is
/// meaningful, and dropping the block there would remove it from the middle of
/// the history.
fn reasoning(block: &Value) -> Reasoning {
    let text = block
        .get("thinking")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let signature = block
        .get("signature")
        .and_then(Value::as_str)
        .map(str::to_string);
    Reasoning {
        id: None,
        content: vec![ReasoningContent::Text {
            text: text.to_string(),
            signature,
        }],
    }
}

fn tool_call(block: &Value) -> Result<ToolCall, ConvertError> {
    let id = block.get("id").and_then(Value::as_str).unwrap_or_default();
    let name = block.get("name").and_then(Value::as_str).unwrap_or_default();
    let arguments = block.get("input").cloned().unwrap_or(Value::Null);

    // `ToolCallId::new` rejects an empty string. An id-less tool call cannot be
    // answered, so this is a real error rather than something to paper over
    // with a minted handle that no `tool_result` will ever match.
    let call = ToolCallId::new(id)
        .ok_or_else(|| ConvertError::new("tool_use", "tool call has no id, so no result can answer it"))?;

    Ok(ToolCall {
        id: call,
        provider: ProviderCallId::new(id),
        function: ToolFunction {
            name: name.to_string(),
            arguments,
        },
        signature: None,
        additional_params: None,
    })
}

fn tool_result(block: &Value) -> Result<ToolResult, ConvertError> {
    let id = block
        .get("tool_use_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let call = ToolCallId::new(id).ok_or_else(|| {
        ConvertError::new(
            "tool_result",
            "tool result has no tool_use_id, so it answers nothing",
        )
    })?;

    // Zest's own producer always writes a bare string body. A block-array body
    // belongs to a path that reads it back defensively, and guessing at its
    // shape here would paper over a real mismatch.
    let text = match block.get("content") {
        Some(Value::String(body)) => body.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    };

    Ok(ToolResult {
        call,
        provider: ProviderCallId::new(id),
        name: String::new(),
        content: vec![ToolResultContent::Text(Text::new(text))],
    })
}

/// Zest's tool schemas as Rig tool definitions.
///
/// `cache_control` is deliberately not carried across. It is a placement
/// decision made against one provider's cache semantics, and re-deriving it on
/// the Rig side is that provider's job, not this seam's.
pub fn to_rig_tools(tools: &[ToolDef]) -> Vec<rig_core::completion::ToolDefinition> {
    tools
        .iter()
        .map(|tool| rig_core::completion::ToolDefinition {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.input_schema.clone(),
        })
        .collect()
}

/// Rig's assistant content as Zest wire blocks.
///
/// The return direction of [`to_rig_history`], and it has to be its inverse:
/// `agent.rs` pushes this straight back into history with
/// `Message::assistant`, and the next turn converts it out again. Anything that
/// does not survive both directions would be dropped on the second trip, which
/// is the failure this seam exists to prevent.
///
/// Tool ids come back as the identifier the provider issued, never rig's minted
/// handle. `agent.rs` matches `tool_result.tool_use_id` against these, and the
/// provider matches them again on the wire, so a synthetic id would break both.
pub fn from_rig_content(content: &[AssistantContent]) -> Vec<Value> {
    let mut out = Vec::with_capacity(content.len());
    for item in content {
        match item {
            AssistantContent::Text(text) => {
                out.push(serde_json::json!({ "type": "text", "text": text.text }))
            }
            AssistantContent::ToolCall(call) => {
                let id = call
                    .provider
                    .as_ref()
                    .map(|provider| provider.call_id.as_str())
                    .unwrap_or_else(|| call.id.as_str());
                out.push(serde_json::json!({
                    "type": "tool_use",
                    "id": id,
                    "name": call.function.name,
                    "input": call.function.arguments,
                }));
            }
            AssistantContent::Reasoning(reasoning) => {
                for part in &reasoning.content {
                    out.push(reasoning_block(part));
                }
            }
            // Rig models assistant images; Zest's history has no place for one
            // and no provider it targets emits them. Skipping keeps the block
            // out of a history that could not replay it anyway.
            AssistantContent::Image(_) => {}
        }
    }
    out
}

fn reasoning_block(part: &ReasoningContent) -> Value {
    match part {
        ReasoningContent::Text { text, signature } => {
            let mut block = serde_json::json!({ "type": "thinking", "thinking": text });
            if let Some(signature) = signature {
                block["signature"] = Value::String(signature.clone());
            }
            block
        }
        ReasoningContent::Redacted { data } => {
            serde_json::json!({ "type": "redacted_thinking", "data": data })
        }
        // An OpenAI Responses encrypted reasoning item. It has no Anthropic
        // equivalent, so it gets its own block type rather than being flattened
        // into `thinking`: the payload is opaque and only round-trips if it
        // stays whole and distinguishable.
        ReasoningContent::Encrypted(data) => {
            serde_json::json!({ "type": "reasoning_encrypted", "data": data })
        }
        // A summary is provider-written prose about the reasoning, not the
        // reasoning itself, so it carries no signature to preserve.
        ReasoningContent::Summary(text) => {
            serde_json::json!({ "type": "thinking", "thinking": text })
        }
    }
}

fn block_type(block: &Value) -> &str {
    block.get("type").and_then(Value::as_str).unwrap_or("")
}

fn text_of(blocks: &[Value]) -> String {
    blocks
        .iter()
        .filter(|block| block_type(block) == "text")
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user(blocks: Vec<Value>) -> Message {
        Message::user_blocks(blocks)
    }

    fn assistant(blocks: Vec<Value>) -> Message {
        Message::assistant(blocks)
    }

    #[test]
    fn a_plain_exchange_keeps_its_order_and_roles() {
        let messages = vec![
            user(vec![json!({"type":"text","text":"hola"})]),
            assistant(vec![json!({"type":"text","text":"qué tal"})]),
        ];
        let (instructions, history) = to_rig_history(None, &messages).unwrap();
        assert!(instructions.is_none());
        assert_eq!(history.len(), 2);
        assert!(matches!(history[0], RigMessage::User { .. }));
        assert!(matches!(history[1], RigMessage::Assistant { .. }));
    }

    #[test]
    fn the_system_prompt_and_system_turns_are_joined_into_instructions() {
        let system = SystemPrompt::new("base").with_volatile("entorno");
        let messages = vec![Message {
            role: "system".into(),
            content: vec![json!({"type":"text","text":"extra"})],
        }];
        let (instructions, history) = to_rig_history(Some(&system), &messages).unwrap();
        let instructions = instructions.expect("system content becomes instructions");
        assert!(instructions.contains("base"), "{instructions}");
        assert!(instructions.contains("entorno"), "{instructions}");
        assert!(instructions.contains("extra"), "{instructions}");
        assert!(history.is_empty(), "a system turn is not chat history");
    }

    /// The identifier the provider issued has to be the one that goes back on
    /// the wire, or the result answers a call the API never made.
    #[test]
    fn a_tool_round_trip_carries_the_provider_id_on_both_halves() {
        let messages = vec![
            assistant(vec![
                json!({"type":"text","text":"leyendo"}),
                json!({"type":"tool_use","id":"toolu_42","name":"read_file","input":{"path":"a.rs"}}),
            ]),
            user(vec![json!({
                "type":"tool_result","tool_use_id":"toolu_42","content":"fn main() {}"
            })]),
        ];
        let (_, history) = to_rig_history(None, &messages).unwrap();

        let RigMessage::Assistant { content, .. } = &history[0] else {
            panic!("expected an assistant turn");
        };
        assert_eq!(content.len(), 2, "text and tool call both survive");
        let AssistantContent::ToolCall(call) = &content[1] else {
            panic!("expected a tool call");
        };
        assert_eq!(call.id.as_str(), "toolu_42");
        assert_eq!(
            call.provider.as_ref().map(|p| p.call_id.as_str()),
            Some("toolu_42"),
            "the provider id must be set or a minted handle reaches the wire"
        );
        assert_eq!(call.function.name, "read_file");
        assert_eq!(call.function.arguments, json!({"path":"a.rs"}));

        let RigMessage::User { content } = &history[1] else {
            panic!("expected a user turn");
        };
        let UserContent::ToolResult(result) = &content[0] else {
            panic!("expected a tool result");
        };
        assert_eq!(result.wire_call_id(), "toolu_42");
        assert_eq!(
            result.content,
            vec![ToolResultContent::Text(Text::new("fn main() {}"))]
        );
    }

    #[test]
    fn a_thinking_block_keeps_its_signature() {
        let messages = vec![assistant(vec![
            json!({"type":"thinking","thinking":"hmm","signature":"sig-abc"}),
            json!({"type":"text","text":"listo"}),
        ])];
        let (_, history) = to_rig_history(None, &messages).unwrap();
        let RigMessage::Assistant { content, .. } = &history[0] else {
            panic!("expected an assistant turn");
        };
        let AssistantContent::Reasoning(reasoning) = &content[0] else {
            panic!("expected reasoning");
        };
        assert_eq!(
            reasoning.content,
            vec![ReasoningContent::Text {
                text: "hmm".into(),
                signature: Some("sig-abc".into()),
            }]
        );
    }

    /// On models where `display` is `"omitted"` the text is empty by design and
    /// only the signature means anything. Dropping the block would remove it
    /// from the middle of the history, which invalidates every later one.
    #[test]
    fn a_thinking_block_with_no_text_still_survives() {
        let messages = vec![assistant(vec![
            json!({"type":"thinking","thinking":"","signature":"sig-only"}),
            json!({"type":"text","text":"listo"}),
        ])];
        let (_, history) = to_rig_history(None, &messages).unwrap();
        let RigMessage::Assistant { content, .. } = &history[0] else {
            panic!("expected an assistant turn");
        };
        assert_eq!(content.len(), 2, "the signature-only block was dropped");
    }

    /// This is the migration's load-bearing guarantee. Rig's `AssistantContent`
    /// has four variants and no passthrough, so a block type it does not model
    /// must surface here rather than vanish.
    #[test]
    fn an_unmodelled_assistant_block_is_an_error_not_a_silent_drop() {
        let messages = vec![assistant(vec![
            json!({"type":"text","text":"buscando"}),
            json!({"type":"server_tool_use","id":"srvtoolu_1","name":"web_search","input":{}}),
        ])];
        let error = to_rig_history(None, &messages).unwrap_err();
        assert_eq!(error.block_type, "server_tool_use");
        assert!(error.to_string().contains("server_tool_use"), "{error}");
    }

    #[test]
    fn a_tool_call_without_an_id_is_refused() {
        let messages = vec![assistant(vec![
            json!({"type":"tool_use","id":"","name":"read_file","input":{}}),
        ])];
        let error = to_rig_history(None, &messages).unwrap_err();
        assert_eq!(error.block_type, "tool_use");
    }

    #[test]
    fn an_empty_message_is_dropped_rather_than_sent() {
        // Rig rejects empty content at the request boundary, so a turn that
        // converts to nothing must not reach it.
        let messages = vec![
            assistant(vec![json!({"type":"text","text":""})]),
            user(vec![json!({"type":"text","text":"sigue"})]),
        ];
        let (_, history) = to_rig_history(None, &messages).unwrap();
        assert_eq!(history.len(), 1);
        assert!(matches!(history[0], RigMessage::User { .. }));
    }

    /// The seam has to be its own inverse. `agent.rs` pushes `from_rig_content`
    /// straight back into history and the next turn converts it out again, so
    /// anything that survives one direction but not the other is a block that
    /// disappears on the second trip.
    #[test]
    fn assistant_content_survives_a_full_round_trip() {
        let original = vec![
            json!({"type":"thinking","thinking":"pensando","signature":"sig-1"}),
            json!({"type":"redacted_thinking","data":"opaco"}),
            json!({"type":"reasoning_encrypted","data":"cifrado"}),
            json!({"type":"text","text":"aquí va"}),
            json!({"type":"tool_use","id":"call_7","name":"grep","input":{"pattern":"fn"}}),
        ];
        let (_, history) = to_rig_history(None, &[assistant(original.clone())]).unwrap();
        let RigMessage::Assistant { content, .. } = &history[0] else {
            panic!("expected an assistant turn");
        };
        let back = from_rig_content(content);
        assert_eq!(back, original);

        // And a second trip changes nothing, which is what makes it safe to
        // replay a history that has already been through the seam once.
        let (_, again) = to_rig_history(None, &[assistant(back.clone())]).unwrap();
        let RigMessage::Assistant { content, .. } = &again[0] else {
            panic!("expected an assistant turn");
        };
        assert_eq!(from_rig_content(content), original);
    }

    #[test]
    fn a_returned_tool_call_carries_the_provider_id_not_rigs_handle() {
        let (_, history) = to_rig_history(
            None,
            &[assistant(vec![
                json!({"type":"tool_use","id":"call_9","name":"read_file","input":{}}),
            ])],
        )
        .unwrap();
        let RigMessage::Assistant { content, .. } = &history[0] else {
            panic!("expected an assistant turn");
        };
        let back = from_rig_content(content);
        assert_eq!(back[0]["id"], "call_9");
    }

    #[test]
    fn tool_schemas_carry_name_description_and_parameters() {
        let tools = vec![ToolDef {
            name: "read_file".into(),
            description: "Read a file".into(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"}}}),
            cache_control: None,
        }];
        let converted = to_rig_tools(&tools);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].name, "read_file");
        assert_eq!(converted[0].description, "Read a file");
        assert_eq!(converted[0].parameters["type"], "object");
    }
}
