//! The ChatGPT Codex turn, served through Rig.
//!
//! Replaces the hand-rolled Responses client in [`super::codex_oauth`]: the
//! request shape, the SSE parse, and the event reassembly all move to
//! `rig_core::providers::chatgpt`, which targets the same
//! `chatgpt.com/backend-api/codex` endpoint.
//!
//! # Zest keeps the credentials
//!
//! Rig can run the whole OAuth device flow and persist tokens to a file of its
//! own. Zest does not use that: the session lives in the OS credential manager
//! and [`crate::codex_oauth::refresh_and_store`] owns the refresh. Rig is
//! handed an already-valid access token through
//! [`ChatGPTAuth::AccessToken`] and never learns where it came from. One
//! credential store, one refresh path, and no second copy of a bearer token on
//! disk.
//!
//! # The model id is Zest's, not Rig's
//!
//! Rig exposes model constants (`gpt-5.3-codex` and friends) that lag Zest's
//! catalogue. `completion_model` takes any string, so
//! [`crate::provider::CODEX_KNOWN_MODELS`] stays the source of truth and the id
//! is passed straight through. Adopting Rig's constants would silently narrow
//! the picker to a stale list.

use futures_util::StreamExt;
use rig_core::client::{ClientBuilder, CompletionClient};
use rig_core::completion::CompletionRequest;
use rig_core::providers::chatgpt::{ChatGPTAuth, ChatGPTBuilder};
use rig_core::streaming::StreamedAssistantContent;
use serde_json::json;

use super::rig_convert::{from_rig_content, to_rig_history, to_rig_tools};
use super::{Completion, StreamEvent, TurnRequest};
use crate::anthropic::types::Usage;
use crate::cancel::wait_cancel;
use crate::codex_oauth::CodexOAuthSession;
use crate::error::{HarnessError, Result};

/// Run one turn and stream it back as Zest events.
///
/// `session` must already be refreshed; this function does not renew it. The
/// caller holds the credential store and is the only place that should write
/// to it.
pub async fn stream_turn(
    session: &CodexOAuthSession,
    req: &TurnRequest,
    on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
) -> Result<Completion> {
    let (preamble, chat_history) = to_rig_history(req.system.as_ref(), &req.messages)
        .map_err(|error| HarnessError::Other(error.to_string()))?;

    let client = ClientBuilder::<ChatGPTBuilder>::default()
        .api_key(ChatGPTAuth::AccessToken {
            access_token: session.access_token.clone(),
            account_id: Some(session.account_id.clone()),
        })
        .build()
        .map_err(|error| HarnessError::Other(format!("ChatGPT client: {error}")))?;

    let model = client.completion_model(req.model.clone());

    // A turn that may not call tools sends no tool list. Unlike Anthropic there
    // is no prompt-prefix cache to keep aligned here, so shipping schemas the
    // model is forbidden to use would be pure cost.
    let tools = if req.allow_tool_use {
        to_rig_tools(&req.tools)
    } else {
        Vec::new()
    };

    let request = CompletionRequest {
        // The id already rides on the model handle; setting it twice lets the
        // two disagree.
        model: None,
        preamble,
        chat_history,
        documents: Vec::new(),
        tools,
        temperature: None,
        max_tokens: Some(u64::from(req.max_tokens)),
        tool_choice: None,
        additional_params: req
            .effort
            .as_ref()
            .filter(|effort| !effort.is_empty())
            .map(|effort| json!({ "reasoning": { "effort": effort } })),
        output_schema: None,
        record_telemetry_content: false,
    };

    let mut stream = model
        .stream(request)
        .await
        .map_err(|error| HarnessError::Other(format!("ChatGPT stream: {error}")))?;

    let mut usage = Usage::default();
    let mut usage_available = false;
    let mut stop_reason: Option<String> = None;
    let mut served_model: Option<String> = None;

    loop {
        let next = tokio::select! {
            biased;
            _ = wait_cancel(req.cancel.as_ref()) => return Err(HarnessError::Cancelled),
            next = stream.next() => next,
        };
        let Some(item) = next else { break };
        let item = item.map_err(|error| HarnessError::Other(format!("ChatGPT stream: {error}")))?;

        match item {
            StreamedAssistantContent::Text(text) => on_event(StreamEvent::Text(&text.text)),
            StreamedAssistantContent::ReasoningDelta { reasoning, .. } => {
                on_event(StreamEvent::Thinking(&reasoning))
            }
            // The completed call is where the provider-issued id lands, and the
            // UI row has to carry that id: `agent.rs` matches the tool result
            // against it and so does the next request.
            StreamedAssistantContent::ToolCall { tool_call, .. } => {
                let id = tool_call
                    .provider
                    .as_ref()
                    .map(|provider| provider.call_id.as_str())
                    .unwrap_or_else(|| tool_call.id.as_str());
                on_event(StreamEvent::ToolCallStart {
                    name: &tool_call.function.name,
                    id,
                });
            }
            StreamedAssistantContent::Final(final_record) => {
                usage = map_usage(&final_record.usage);
                usage_available = true;
                stop_reason = final_record.finish_reason.as_ref().map(stop_reason_of);
                served_model = final_record.model.clone();
            }
            // Reasoning replaces its deltas rather than adding to them, and the
            // aggregated `choice` already applies that. Emitting here would
            // duplicate the text the deltas already streamed.
            StreamedAssistantContent::Reasoning { .. } => {}
            // Partial tool arguments. The UI row opens on the completed call,
            // so there is nothing to show yet.
            StreamedAssistantContent::ToolCallDelta { .. } => {}
            // A provider-native item Rig does not model. It is deliberately not
            // added to the accumulated turn, so there is nothing to persist.
            StreamedAssistantContent::Unknown(_) => {}
        }
    }

    let content = from_rig_content(&stream.choice);

    // `agent.rs` drives its loop off this: `tool_use` means run the tools and
    // come back, anything else ends the turn. A provider that reported no
    // finish reason but did ask for a tool must still say so.
    let stop_reason = match stop_reason {
        Some(reason) => Some(reason),
        None if content
            .iter()
            .any(|block| block.get("type").and_then(|kind| kind.as_str()) == Some("tool_use")) =>
        {
            Some("tool_use".to_string())
        }
        None => Some("end_turn".to_string()),
    };

    Ok(Completion {
        content,
        stop_reason,
        usage,
        usage_available,
        limits: None,
        served_model,
        provider_session: None,
    })
}

/// Rig's finish reason as the `stop_reason` string `agent.rs` matches on.
fn stop_reason_of(reason: &rig_core::completion::FinishReason) -> String {
    use rig_core::completion::FinishReason;
    match reason {
        FinishReason::ToolCalls => "tool_use".to_string(),
        FinishReason::Length => "max_tokens".to_string(),
        FinishReason::ContentFilter => "refusal".to_string(),
        // `Stop` and anything Rig adds later both mean "the turn is over", which
        // is what `end_turn` tells the agent loop.
        _ => "end_turn".to_string(),
    }
}

/// Rig's usage as Zest's.
///
/// The Responses backend reports a cached-token count inside its input total,
/// the same convention the hand-rolled client normalized. Zest's
/// [`Usage::prompt_tokens`] sums the columns, so the cached share is subtracted
/// out of `input_tokens` rather than counted twice.
fn map_usage(usage: &rig_core::completion::Usage) -> Usage {
    let cached = u32::try_from(usage.cached_input_tokens).unwrap_or(u32::MAX);
    let input = u32::try_from(usage.input_tokens).unwrap_or(u32::MAX);
    Usage {
        input_tokens: input.saturating_sub(cached),
        output_tokens: u32::try_from(usage.output_tokens).unwrap_or(u32::MAX),
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: cached,
    }
}
