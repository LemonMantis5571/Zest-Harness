//! Provider backed by the Messages API.
//!
//! Serves two cases from one implementation:
//!
//! - **Native** — Anthropic's API on an Anthropic key.
//! - **Gateway** — a user-run proxy (LiteLLM, …) that re-exposes some other
//!   backend as the Messages API. Zest neither installs nor supervises one; a
//!   backend reached this way needs no second wire protocol in the harness.
//!
//! The only behavioural difference is whether Anthropic-only request fields are
//! sent. That decision belongs here rather than in the agent loop, which is why
//! `Agent` no longer carries a flag for it.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{catalogue_from_lists, Completion, ModelSpec, Provider, StreamEvent, TurnRequest};
use crate::anthropic::client::AnthropicClient;
use crate::anthropic::types::{
    cached_system_blocks, ephemeral_cache_control, long_cache_control, Message, OutputConfig,
    Request, Thinking, ToolDef, DEFAULT_MODEL,
};
use crate::auth::AuthStatus;
use crate::error::Result;

pub struct AnthropicProvider {
    id: String,
    client: AnthropicClient,
    default_model: String,
    models: Vec<ModelSpec>,
    /// Presence only — the key itself is never inspected or reported.
    has_key: bool,
    /// Whether the endpoint understands `thinking` and `output_config.effort`.
    /// False behind a gateway fronting a non-Anthropic model: those fields are
    /// meaningless there, and are dropped or rejected depending on the proxy.
    extensions: bool,
}

impl AnthropicProvider {
    /// Anthropic's own API.
    pub fn native(api_key: String) -> Result<Self> {
        let has_key = !api_key.trim().is_empty();
        let default_model = DEFAULT_MODEL.to_string();
        Ok(Self {
            id: "anthropic".to_string(),
            client: AnthropicClient::new(api_key)?,
            models: catalogue_from_lists(&default_model, &[], &[]),
            default_model,
            has_key,
            extensions: true,
        })
    }

    /// A Messages-API-speaking gateway in front of some other backend.
    ///
    /// `id` is what configuration and the usage ledger key on, so it should name
    /// the *account* being spent (`"codex"`), not the proxy.
    pub fn gateway(
        id: impl Into<String>,
        api_key: String,
        base_url: impl Into<String>,
        default_model: impl Into<String>,
    ) -> Result<Self> {
        let has_key = !api_key.trim().is_empty();
        let default_model = default_model.into();
        Ok(Self {
            id: id.into(),
            client: AnthropicClient::new(api_key)?.with_base_url(base_url),
            // No optional catalogue → only the configured default is accepted.
            models: catalogue_from_lists(&default_model, &[], &[]),
            default_model,
            has_key,
            extensions: false,
        })
    }

    /// Name this provider after the account it spends, not the transport.
    /// Configuration and the usage ledger key on this.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    pub fn with_default_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = model.into();
        if !self.models.iter().any(|m| m.id == self.default_model) {
            let efforts = self
                .models
                .first()
                .map(|m| m.efforts.clone())
                .unwrap_or_else(|| {
                    super::STANDARD_EFFORTS
                        .iter()
                        .map(|s| (*s).to_string())
                        .collect()
                });
            self.models.insert(
                0,
                ModelSpec {
                    id: self.default_model.clone(),
                    efforts,
                    context_window: super::context_window_for_model(&self.default_model),
                    supports_tools: true,
                    supports_vision: false,
                },
            );
        }
        self
    }

    /// Replace the model/effort catalogue (from gateway config allow-lists).
    pub fn with_models(mut self, models: Vec<ModelSpec>) -> Self {
        self.models = models;
        self
    }

    /// Override whether Anthropic extensions are sent.
    ///
    /// Only needed for a gateway that genuinely fronts an Anthropic model and can
    /// pass the fields through — the constructors already pick the right default.
    pub fn with_extensions(mut self, extensions: bool) -> Self {
        self.extensions = extensions;
        self
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn models(&self) -> Vec<ModelSpec> {
        self.models.clone()
    }

    /// This provider authenticates with a key it was handed, so its status is
    /// simply whether it has one. Providers backed by a vendor CLI sign-in
    /// report from `crate::auth` detection instead.
    fn auth_status(&self) -> AuthStatus {
        if self.has_key {
            AuthStatus::Ready { account: None }
        } else {
            AuthStatus::Unconfigured
        }
    }

    /// Native Anthropic only. `extensions` already means "this really is the
    /// Messages API, not a translation layer", which is exactly the condition
    /// under which `cache_control` means anything.
    fn supports_prompt_cache(&self) -> bool {
        self.extensions
    }

    async fn stream_turn(
        &self,
        req: &TurnRequest,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    ) -> Result<Completion> {
        let caching = self.supports_prompt_cache();

        let (tools, tool_choice) = tool_plan(req, caching);

        let system = req.system.as_ref().map(|prompt| {
            if caching {
                // A second breakpoint here extends the cached region to cover
                // tools + system, which together are stable for a whole
                // session. The environment half stays outside it.
                cached_system_blocks(&prompt.cacheable, &prompt.volatile)
            } else {
                Value::String(prompt.text())
            }
        });

        let mut messages = req.messages.clone();
        if caching {
            mark_conversation_prefix(&mut messages);
        }

        let wire = Request {
            model: req.model.clone(),
            max_tokens: req.max_tokens,
            stream: true,
            system,
            messages,
            tools,
            tool_choice,
            thinking: (self.extensions && req.thinking).then(Thinking::default),
            output_config: match (self.extensions, req.effort.as_ref()) {
                (true, Some(effort)) => Some(OutputConfig {
                    effort: effort.clone(),
                }),
                _ => None,
            },
        };

        self.client
            .stream_cancellable(&wire, on_event, req.cancel.as_ref())
            .await
    }
}

/// What goes on the wire for `tools`, and whether the model may call any of it.
///
/// A turn that may not call tools still carries the list **when caching**,
/// purely so its prefix matches a normal turn's and hits the same cache.
/// Without caching there is no prefix to match, so shipping a tool schema the
/// model is forbidden to use is pure cost — and leaves a gateway that quietly
/// drops `tool_choice` free to call one anyway, which for compaction means a
/// `tool_use` reply where text was required.
///
/// `tool_choice` is gated on the list actually being present. `Request.tools`
/// is skipped when empty, so an ungated choice would constrain a tool list the
/// body never sent — the exact shape `probe` and the reading-diff pass produce.
fn tool_plan(req: &TurnRequest, caching: bool) -> (Vec<ToolDef>, Option<Value>) {
    let mut tools = if req.allow_tool_use || caching {
        req.tools.clone()
    } else {
        Vec::new()
    };
    // One breakpoint on the last tool covers the entire tool list — the largest
    // fixed prefix any request has, and fixed for the whole session, so it
    // earns the long TTL.
    if caching {
        if let Some(last) = tools.last_mut() {
            last.cache_control = Some(long_cache_control());
        }
    }
    let tool_choice = (!req.allow_tool_use && !tools.is_empty()).then(|| json!({ "type": "none" }));
    (tools, tool_choice)
}

/// Put rolling breakpoints near the end of the conversation so the history that
/// already exists is read from cache instead of reprocessed every turn.
///
/// The newest goes on the **second-to-last** message, not the last. The last
/// message is the one that just changed; a breakpoint there would write a new
/// cache entry every turn and read none of it back. One message earlier is the
/// newest point that was also present on the previous request.
///
/// A second breakpoint goes on the message before that, using the last of the
/// four the API allows. A cache lookup only walks a bounded number of content
/// blocks backwards before giving up, and one round of a tool-calling turn —
/// a thinking block, N `tool_use`, then N `tool_result` — can exceed that
/// window on its own when the model fans out. With a single breakpoint the
/// whole conversation then silently re-reads at full price on exactly the
/// turns that are most expensive. Two adjacent breakpoints bound the gap to a
/// single message's worth of blocks, and cost nothing extra: each entry only
/// stores the delta past the one before it.
fn mark_conversation_prefix(messages: &mut [Message]) {
    let Some(newest) = markable_before(messages, messages.len().saturating_sub(1)) else {
        return;
    };
    mark(&mut messages[newest]);
    if let Some(older) = markable_before(messages, newest) {
        mark(&mut messages[older]);
    }
}

/// Nearest message strictly before `end` whose last content block can carry a
/// breakpoint, or `None`.
///
/// Walks rather than indexing because a message is not always markable, and a
/// breakpoint that silently fails to land is a full-price turn nobody notices.
fn markable_before(messages: &[Message], end: usize) -> Option<usize> {
    messages[..end]
        .iter()
        .rposition(|message| message.content.last().is_some_and(markable))
}

/// Only object-shaped blocks take `cache_control`. Thinking blocks carry a
/// signature that must round-trip byte for byte, so never touch those.
fn markable(block: &Value) -> bool {
    block
        .as_object()
        .is_some_and(|map| map.get("type").and_then(Value::as_str) != Some("thinking"))
}

fn mark(message: &mut Message) {
    if let Some(map) = message.content.last_mut().and_then(Value::as_object_mut) {
        map.insert("cache_control".into(), ephemeral_cache_control());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::SystemPrompt;
    use serde_json::json;

    fn conversation(n: usize) -> Vec<Message> {
        (0..n)
            .map(|i| {
                if i % 2 == 0 {
                    Message::user_text(format!("u{i}"))
                } else {
                    Message::assistant(vec![json!({ "type": "text", "text": format!("a{i}") })])
                }
            })
            .collect()
    }

    fn cached_indices(messages: &[Message]) -> Vec<usize> {
        messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.content.iter().any(|b| b.get("cache_control").is_some()))
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn rolling_breakpoints_land_on_the_two_messages_before_the_last() {
        // The last message is the one that just changed. A breakpoint there
        // would write a fresh entry every turn and never read one back. The
        // pair keeps the gap a lookup has to walk down to one message.
        let mut messages = conversation(5);
        mark_conversation_prefix(&mut messages);
        assert_eq!(cached_indices(&messages), vec![2, 3]);
    }

    #[test]
    fn a_short_conversation_takes_the_breakpoints_it_can() {
        let mut messages = conversation(2);
        mark_conversation_prefix(&mut messages);
        assert_eq!(cached_indices(&messages), vec![0]);

        let mut messages = conversation(1);
        mark_conversation_prefix(&mut messages);
        assert!(cached_indices(&messages).is_empty());

        let mut empty: Vec<Message> = Vec::new();
        mark_conversation_prefix(&mut empty);
    }

    #[test]
    fn a_thinking_block_is_never_annotated() {
        // Thinking blocks carry a signature that must echo back byte for byte;
        // adding a key to one would invalidate the next request.
        let mut messages = vec![
            Message::assistant(vec![
                json!({ "type": "thinking", "thinking": "hmm", "signature": "sig" }),
            ]),
            Message::user_text("next"),
        ];
        mark_conversation_prefix(&mut messages);
        assert!(cached_indices(&messages).is_empty());
        assert_eq!(messages[0].content[0]["signature"], "sig");
    }

    /// The old placement indexed straight to `len - 2` and gave up if that one
    /// message happened to end in a thinking block, so the turn silently ran
    /// with no conversation breakpoint at all.
    #[test]
    fn an_unmarkable_message_is_stepped_over_rather_than_skipping_the_turn() {
        let mut messages = vec![
            Message::user_text("first"),
            Message::assistant(vec![json!({ "type": "text", "text": "answer" })]),
            Message::assistant(vec![
                json!({ "type": "thinking", "thinking": "hmm", "signature": "sig" }),
            ]),
            Message::user_text("latest"),
        ];
        mark_conversation_prefix(&mut messages);
        assert_eq!(cached_indices(&messages), vec![0, 1]);
        assert_eq!(messages[2].content[0]["signature"], "sig");
    }

    /// A message with no content blocks has nowhere to put a breakpoint; the
    /// walk has to keep going rather than treat it as the answer.
    #[test]
    fn an_empty_message_is_stepped_over() {
        let mut messages = vec![
            Message::user_text("first"),
            Message::assistant(Vec::new()),
            Message::user_text("latest"),
        ];
        mark_conversation_prefix(&mut messages);
        assert_eq!(cached_indices(&messages), vec![0]);
    }

    #[test]
    fn gateway_sends_a_plain_string_system_and_no_cache_control() {
        let provider =
            AnthropicProvider::gateway("codex", "k".into(), "http://x", "gpt-5.6-sol").unwrap();
        assert!(!provider.supports_prompt_cache());
    }

    #[test]
    fn native_provider_reports_cache_support() {
        let provider = AnthropicProvider::native("k".into()).unwrap();
        assert!(provider.supports_prompt_cache());
    }

    /// Serializing the wire request is the only way to prove the shape the API
    /// actually receives, including that untouched fields stay absent.
    #[test]
    fn cached_system_serializes_as_a_block_array() {
        let plain = Request {
            model: "m".into(),
            max_tokens: 1,
            stream: true,
            system: Some(Value::String("hello".into())),
            messages: Vec::new(),
            tools: Vec::new(),
            tool_choice: None,
            thinking: None,
            output_config: None,
        };
        let json = serde_json::to_value(&plain).unwrap();
        assert_eq!(json["system"], json!("hello"));
        assert!(json.get("tool_choice").is_none());

        let cached = Request {
            system: Some(cached_system_blocks("hello", "")),
            ..plain
        };
        let json = serde_json::to_value(&cached).unwrap();
        assert_eq!(json["system"][0]["text"], json!("hello"));
        assert_eq!(json["system"].as_array().unwrap().len(), 1);
        assert_eq!(
            json["system"][0]["cache_control"]["type"],
            json!("ephemeral")
        );
        // The session-stable half of the prompt outlives the default five
        // minutes, which is shorter than the pause between two human turns.
        assert_eq!(json["system"][0]["cache_control"]["ttl"], json!("1h"));
    }

    /// The environment block reports the git branch, so it changes between
    /// sessions in one project. Folded into the cached text it would take the
    /// base prompt, project docs, and every skill description down with it.
    #[test]
    fn the_environment_block_sits_after_the_breakpoint_not_inside_it() {
        let json = serde_json::to_value(cached_system_blocks("stable", "# Environment\nbranch x"))
            .unwrap();
        let blocks = json.as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["text"], json!("stable"));
        assert!(blocks[0].get("cache_control").is_some());
        assert_eq!(blocks[1]["text"], json!("# Environment\nbranch x"));
        assert!(
            blocks[1].get("cache_control").is_none(),
            "a breakpoint here would defeat the split: {json}"
        );

        // Changing only the volatile half must leave the cached block's bytes
        // untouched, which is the whole point of the arrangement.
        let moved = serde_json::to_value(cached_system_blocks("stable", "# Environment\nbranch y"))
            .unwrap();
        assert_eq!(moved.as_array().unwrap()[0], blocks[0]);
    }

    fn turn(tools: Vec<ToolDef>, allow_tool_use: bool) -> TurnRequest {
        TurnRequest {
            model: "m".into(),
            system: None,
            messages: Vec::new(),
            tools,
            allow_tool_use,
            max_tokens: 16,
            effort: None,
            thinking: false,
            provider_session: None,
            interaction: None,
            cancel: None,
        }
    }

    fn one_tool() -> Vec<ToolDef> {
        vec![ToolDef {
            name: "t".into(),
            description: "d".into(),
            input_schema: json!({}),
            cache_control: None,
        }]
    }

    /// `Request.tools` is skipped when empty, so an ungated `tool_choice` would
    /// constrain a list the body never sent. `probe` and the reading-diff pass
    /// are exactly that shape: no tools, and no tool use either.
    #[test]
    fn a_tool_less_request_sends_no_tool_choice() {
        let (tools, choice) = tool_plan(&turn(Vec::new(), false), true);
        assert!(tools.is_empty());
        assert_eq!(
            choice, None,
            "a tool_choice with no tools constrains a list that was never sent"
        );

        // And prove it stays absent from the serialized body, alongside `tools`.
        let wire = Request {
            model: "m".into(),
            max_tokens: 1,
            stream: true,
            system: None,
            messages: Vec::new(),
            tools,
            tool_choice: choice,
            thinking: None,
            output_config: None,
        };
        let json = serde_json::to_value(&wire).unwrap();
        assert!(json.get("tools").is_none());
        assert!(json.get("tool_choice").is_none(), "{json}");
    }

    #[test]
    fn a_maintenance_turn_keeps_the_tools_but_forbids_calling_them() {
        // Caching on: the list is what makes this prompt share the session's
        // cached prefix, so it stays and `none` does the forbidding.
        let (tools, choice) = tool_plan(&turn(one_tool(), false), true);
        assert_eq!(tools.len(), 1);
        assert_eq!(choice, Some(json!({ "type": "none" })));
        assert_eq!(
            tools[0].cache_control,
            Some(long_cache_control()),
            "the tool list is session-stable, so it takes the long TTL"
        );
    }

    /// A gateway writes no breakpoints at all, so there is no prefix for the
    /// tool list to match — sending a schema the model may not use is pure
    /// cost, and one the gateway may not honour `none` for.
    #[test]
    fn a_maintenance_turn_withholds_tools_from_a_provider_that_cannot_cache() {
        let (tools, choice) = tool_plan(&turn(one_tool(), false), false);
        assert!(tools.is_empty());
        assert_eq!(choice, None);
    }

    #[test]
    fn a_normal_turn_keeps_its_tools_and_leaves_the_choice_open() {
        for caching in [true, false] {
            let (tools, choice) = tool_plan(&turn(one_tool(), true), caching);
            assert_eq!(tools.len(), 1, "caching={caching}");
            assert_eq!(choice, None, "caching={caching}");
        }
    }

    #[test]
    fn char_len_matches_the_rendered_prompt() {
        let prompt = SystemPrompt::new("stable").with_volatile("# Environment");
        assert_eq!(prompt.char_len(), prompt.text().chars().count());
        assert_eq!(
            SystemPrompt::new("stable").char_len(),
            "stable".chars().count()
        );
        // Multi-byte content must count characters, not bytes, to match
        // `text().chars().count()`.
        let wide = SystemPrompt::new("héllo").with_volatile("wörld");
        assert_eq!(wide.char_len(), wide.text().chars().count());
        assert_eq!(SystemPrompt::default().char_len(), 0);
    }

    #[test]
    fn a_gateway_receives_both_halves_as_one_string() {
        // No breakpoints there, so the split must not change what the model
        // reads — only where the boundary falls for a provider that caches.
        let prompt = SystemPrompt::new("stable").with_volatile("# Environment");
        assert_eq!(prompt.text(), "stable\n\n# Environment");
        assert_eq!(SystemPrompt::new("stable").text(), "stable");
    }

    #[test]
    fn tool_defs_omit_cache_control_entirely_when_unset() {
        let def = crate::anthropic::types::ToolDef {
            name: "t".into(),
            description: "d".into(),
            input_schema: json!({}),
            cache_control: None,
        };
        let json = serde_json::to_value(&def).unwrap();
        assert!(
            json.get("cache_control").is_none(),
            "a gateway must not see the field at all: {json}"
        );
    }
}
