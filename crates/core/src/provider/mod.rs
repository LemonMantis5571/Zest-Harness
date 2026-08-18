//! Provider abstraction.
//!
//! A provider is one authenticated backend: Anthropic on a Claude login, Codex on
//! a ChatGPT login, or an explicitly configured OpenAI-compatible endpoint. The
//! agent loop must not know which backend it is talking to.
//!
//! Crucially, *how* a provider is reached is an implementation detail behind this
//! trait. Anthropic is reached natively; another backend may be reached through a
//! gateway that re-exposes it as the Messages API. The agent loop cannot tell the
//! difference, which lets a gateway be swapped for a native client later without
//! anything above noticing.

pub mod anthropic;
pub mod claude_code;
pub(crate) mod claude_control;
pub mod codex_app_server;
pub mod driver;
pub mod openai_compatible;
pub mod registry;
pub mod session;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use crate::anthropic::types::{Message, ToolDef, Usage, DEFAULT_MODEL};
use crate::auth::AuthStatus;
use crate::config::ProviderConfig;
use crate::error::Result;

/// Build a picker/validation catalogue from config without loading credentials.
///
/// This used to be a second exhaustive match over [`ProviderConfig`], and it had
/// drifted from the one in `registry::build`. Both now go through the same
/// driver, so a picker that offers a model the provider rejects is no longer
/// expressible.
pub fn descriptor_from_config(provider_id: &str, config: &ProviderConfig) -> ProviderDescriptor {
    driver::driver_for(config).descriptor(provider_id, config)
}

/// Fallback catalogue when a picker id is *not* present in `zest.toml`.
///
/// The one place a provider id legitimately decides anything: there is no
/// config entry, so there is no kind to ask. Everything else goes through
/// [`driver::driver_for`].
pub fn descriptor_for_picker_id(provider_id: &str) -> ProviderDescriptor {
    let (default_model, builtin) = match provider_id {
        "codex" => ("gpt-5.6-sol".to_string(), CODEX_KNOWN_MODELS),
        "claude" | "anthropic" => (DEFAULT_MODEL.to_string(), &[][..]),
        "antigravity" => ("gemini-3.1-pro-high".to_string(), &[][..]),
        _ => (DEFAULT_MODEL.to_string(), &[][..]),
    };
    ProviderDescriptor {
        id: provider_id.to_string(),
        default_model: default_model.clone(),
        models: catalogue(&default_model, &[], builtin, EffortPolicy::Standard(&[])),
    }
}

/// Efforts every provider understands today.
pub const STANDARD_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// One selectable model and the efforts it accepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSpec {
    pub id: String,
    /// When non-empty, only these efforts are valid for this model.
    pub efforts: Vec<String>,
    /// Conservative context window used by the desktop meter. Providers can
    /// still report a measured input count; this is only the model catalogue's
    /// capacity hint.
    #[serde(default)]
    pub context_window: u64,
    /// Whether the model catalogue expects standard function/tool definitions.
    #[serde(default = "default_true")]
    pub supports_tools: bool,
    /// Whether the catalogue explicitly opts into image input.
    #[serde(default)]
    pub supports_vision: bool,
}

fn default_true() -> bool {
    true
}

/// Conservative built-in capacities for the models Zest knows without model
/// discovery. Explicit provider catalogues remain authoritative for model ids;
/// these values only keep the UI honest when no capacity was configured.
pub fn context_window_for_model(model: &str) -> u64 {
    let model = model.to_ascii_lowercase();
    if model.contains("gpt-5.6") || model.contains("luna") || model.contains("codex") {
        256_000
    } else if model.contains("claude") {
        200_000
    } else {
        128_000
    }
}

fn model_spec(id: String, efforts: Vec<String>) -> ModelSpec {
    ModelSpec {
        context_window: context_window_for_model(&id),
        id,
        efforts,
        supports_tools: true,
        supports_vision: false,
    }
}

/// Static catalogue a provider exposes for pickers and session validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDescriptor {
    pub id: String,
    pub default_model: String,
    pub models: Vec<ModelSpec>,
}

/// Normalize UI / env effort aliases to the wire form.
pub fn normalize_effort(effort: &str) -> String {
    match effort.trim().to_ascii_lowercase().as_str() {
        "low" | "medium" | "high" | "xhigh" | "max" => effort.trim().to_ascii_lowercase(),
        "extra" | "extra high" | "extra_high" => "xhigh".into(),
        "med" => "medium".into(),
        _ => "high".into(),
    }
}

/// Built-in Codex catalogue used when `zest.toml` omits `models` for provider `codex`.
///
/// Mirrors the desktop picker (`CODEX_MODELS` in the UI). Keep these in sync.
pub const CODEX_KNOWN_MODELS: &[&str] = &[
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.5",
    "gpt-5.4",
    "gpt-5.4-mini",
];

/// Whether a provider exposes a reasoning-effort selector, and which levels.
///
/// This replaces a pair of near-identical builders whose only difference was
/// whether they hard-coded an empty effort list. Naming the policy makes the
/// asymmetry a decision rather than a choice of function.
#[derive(Debug, Clone, Copy)]
pub enum EffortPolicy<'a> {
    /// The wire protocol carries no effort field, so advertising a selector
    /// would look authoritative while changing nothing.
    Unsupported,
    /// [`STANDARD_EFFORTS`], narrowed by a non-empty per-provider allow-list.
    Standard(&'a [String]),
}

impl EffortPolicy<'_> {
    fn levels(self) -> Vec<String> {
        match self {
            Self::Unsupported => Vec::new(),
            Self::Standard([]) => STANDARD_EFFORTS.iter().map(|s| (*s).to_string()).collect(),
            Self::Standard(allowed) => allowed.to_vec(),
        }
    }
}

/// The one catalogue builder.
///
/// `models` is the entry's allow-list. When it is empty the provider's own
/// `builtin` ids are used, and when *that* is empty too, only `default_model` is
/// accepted. `default_model` is always present and always first, because a
/// configured default the catalogue rejects is a startup failure.
///
/// `builtin` is passed in rather than looked up. It used to be selected by
/// matching `provider_id == "codex"`, so a `codex_cli` entry under any other
/// name silently lost its catalogue, and an unrelated provider *named* `codex`
/// silently gained one. Capability belongs to the kind, not to the id.
pub fn catalogue(
    default_model: &str,
    models: &[String],
    builtin: &[&str],
    efforts: EffortPolicy<'_>,
) -> Vec<ModelSpec> {
    let levels = efforts.levels();
    let mut ids: Vec<String> = if !models.is_empty() {
        models.to_vec()
    } else if !builtin.is_empty() {
        builtin.iter().map(|id| (*id).to_string()).collect()
    } else {
        vec![default_model.to_string()]
    };
    if !ids.iter().any(|id| id == default_model) {
        ids.insert(0, default_model.to_string());
    }
    ids.into_iter()
        .map(|id| model_spec(id, levels.clone()))
        .collect()
}

fn validate_against(
    models: &[ModelSpec],
    provider_id: &str,
    model: &str,
    effort: &str,
) -> std::result::Result<(), String> {
    let spec = models.iter().find(|m| m.id == model).ok_or_else(|| {
        let known: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
        format!(
            "model `{model}` is not supported by provider `{provider_id}` (known: {})",
            known.join(", ")
        )
    })?;
    if !spec.efforts.is_empty() && !spec.efforts.iter().any(|e| e == effort) {
        return Err(format!(
            "effort `{effort}` is not supported for model `{model}` on provider `{provider_id}` (known: {})",
            spec.efforts.join(", ")
        ));
    }
    Ok(())
}

/// The system prompt, split at the only place a cache breakpoint can sit.
///
/// `cacheable` is fixed for the whole session and is the largest stable block
/// of any request. `volatile` describes the machine the session runs on —
/// working directory, git branch, top-level tree — which differs between
/// sessions in the same project. Kept in one string the two are indivisible,
/// so switching a branch and reopening a thread throws away the entire system
/// prompt to re-report one line of it.
///
/// Splitting them cannot make the volatile half free: it still precedes every
/// message, so a change there costs the conversation prefix. What it buys is
/// the part worth protecting — the base prompt, project docs, and skills all
/// survive, and those dwarf the environment block.
///
/// Providers with no notion of a breakpoint render the two in order via
/// [`Self::text`] and see exactly what they saw before.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemPrompt {
    pub cacheable: String,
    /// Rendered after `cacheable`, past any breakpoint. Often empty.
    pub volatile: String,
}

impl SystemPrompt {
    /// A prompt that is stable all the way through, which is the common case
    /// for everything except a project-aware session.
    pub fn new(cacheable: impl Into<String>) -> Self {
        Self {
            cacheable: cacheable.into(),
            volatile: String::new(),
        }
    }

    pub fn with_volatile(mut self, volatile: impl Into<String>) -> Self {
        self.volatile = volatile.into();
        self
    }

    /// Both halves as one string, in the order the model reads them.
    pub fn text(&self) -> String {
        if self.volatile.is_empty() {
            return self.cacheable.clone();
        }
        format!("{}\n\n{}", self.cacheable, self.volatile)
    }

    /// Characters in [`Self::text`], without building it.
    ///
    /// The context meter needs the length on every refresh and nothing else;
    /// rendering tens of kilobytes of prompt to count them and dropping the
    /// copy a line later is a cost with no reader.
    pub fn char_len(&self) -> usize {
        let separator = if self.volatile.is_empty() { 0 } else { 2 };
        self.cacheable.chars().count() + separator + self.volatile.chars().count()
    }

    pub fn is_empty(&self) -> bool {
        self.cacheable.is_empty() && self.volatile.is_empty()
    }
}

impl From<String> for SystemPrompt {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl From<&str> for SystemPrompt {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

/// One model turn, described without reference to any provider's wire format.
///
/// `effort` and `thinking` are *requests*, not commands. A provider maps them
/// onto its own controls or ignores them — which is why the agent loop no longer
/// carries a flag for whether the backend understands Anthropic's extensions.
pub struct TurnRequest {
    pub model: String,
    pub system: Option<SystemPrompt>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDef>,
    /// Whether the model may actually call anything in `tools`.
    ///
    /// Maintenance turns — compaction, summarisation — want the tool list on
    /// the wire so their prompt prefix still matches a normal turn and hits the
    /// same cache, while never invoking a tool. Sending an empty list instead
    /// would change the prompt from its first byte and cost a second full-price
    /// copy of the prefix. Providers that run their own tool loop ignore this.
    pub allow_tool_use: bool,
    /// Budgets reasoning *and* response text together on providers that think.
    pub max_tokens: u32,
    pub effort: Option<String>,
    pub thinking: bool,
    /// Provider-owned continuation cursor. Only providers that explicitly
    /// support a durable session may persist one; it never contains auth.
    pub provider_session: Option<ProviderSessionRef>,
    /// Host for provider-originated approvals/questions. A missing host means
    /// the provider must fail closed for interactive requests.
    pub interaction: Option<Arc<dyn ProviderInteractionHost>>,
    /// When set, the provider races the HTTP/SSE work against this token and
    /// aborts the body on cancel (drop).
    pub cancel: Option<crate::cancel::CancelToken>,
}

/// Non-secret provider session reference persisted with a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderSessionRef {
    CodexAppServer { thread_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCommandRequest {
    pub approval_id: String,
    pub command: String,
    pub cwd: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderFileChangeRequest {
    pub approval_id: String,
    pub path: Option<String>,
    pub diff: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderQuestionRequest {
    pub id: String,
    pub prompt: String,
    pub choices: Vec<String>,
    pub multiple: bool,
}

/// Narrow interaction seam shared by native CLI providers and the desktop.
/// Implementations must return `false`/`None` when the request cannot be
/// represented safely; providers must never guess an approval.
#[async_trait]
pub trait ProviderInteractionHost: Send + Sync {
    async fn prepare_command_approval(&self, _approval_id: &str) {}

    async fn approve_command(&self, _request: ProviderCommandRequest) -> bool {
        false
    }

    async fn prepare_file_change_approval(&self, _approval_id: &str) {}

    async fn approve_file_change(&self, _request: ProviderFileChangeRequest) -> bool {
        false
    }

    async fn prepare_question(&self, _question_id: &str) {}

    async fn answer_question(&self, _request: ProviderQuestionRequest) -> Option<Vec<String>> {
        None
    }
}

/// Whether a provider can continue a durable stream after the process that
/// started it has gone away. Existing providers default to unsupported until
/// their wire protocol exposes a real, non-secret resume mechanism.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeSupport {
    #[default]
    Unsupported,
    ProviderManaged,
}

/// Provider-owned, non-secret cursor for a durable stream.
///
/// Providers must never place credentials or bearer material in this record.
/// It is persisted with the run so a future resume implementation can identify
/// the provider-side stream without changing the thread transcript format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeHandle {
    pub provider_run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

impl ResumeHandle {
    pub fn new(provider_run_id: impl Into<String>) -> Self {
        Self {
            provider_run_id: provider_run_id.into(),
            cursor: None,
        }
    }

    pub fn with_cursor(mut self, cursor: impl Into<String>) -> Self {
        self.cursor = Some(cursor.into());
        self
    }
}

/// Incremental output, for rendering. Everything here is also present in the
/// final `Completion` — a front-end that ignores these still gets a correct turn.
#[derive(Debug)]
pub enum StreamEvent<'a> {
    Text(&'a str),
    Thinking(&'a str),
    /// Activity owned by a provider's internal agent loop.
    ///
    /// This is intentionally distinct from `ToolCallStart`/`ToolCallResult`:
    /// those variants are Zest tool lifecycle events and cause the Agent to
    /// coordinate local tool execution. Provider-owned activity is display
    /// metadata only; the provider remains responsible for running it.
    ProviderActivity {
        id: &'a str,
        title: &'a str,
        status: &'a str,
    },
    ToolCallStart {
        name: &'a str,
        id: &'a str,
    },
    /// UI-only metadata becomes available after the complete tool input is
    /// prepared, before approval or execution begins.
    ToolCallUpdate {
        name: &'a str,
        id: &'a str,
        metadata: crate::tools::ToolMetadata,
    },
    /// Emitted after a local tool finishes. `summary` is a short preview of the body.
    /// `metadata` is a typed UI/persist side-channel (never model wire content).
    ToolCallResult {
        name: &'a str,
        id: &'a str,
        summary: &'a str,
        is_error: bool,
        path: Option<&'a str>,
        diff: Option<&'a str>,
        metadata: Option<crate::tools::ToolMetadata>,
    },
    /// A provider-independent interactive question requested by the model.
    /// The agent waits for the front-end answer before emitting the matching
    /// tool result and continuing the turn.
    QuestionNeeded {
        question_id: String,
        tool_call_id: String,
        prompt: String,
        choices: Vec<String>,
        multiple: bool,
        placeholder: Option<String>,
    },
    /// The endpoint served a different model than the one asked for.
    ///
    /// Emitted at most once per turn and **only on disagreement** — silence
    /// means the request was honoured. Worth surfacing because nothing else can
    /// tell you: a gateway may route anywhere, and a model's own account of
    /// which model it is amounts to a guess.
    ModelSubstituted {
        requested: String,
        served: String,
    },
    /// Provider checkpoint for a future durable-resume implementation. This is
    /// persistence metadata, not user-visible chat content.
    ResumeHandle(ResumeHandle),
    /// A gated tool is waiting on the user (write/exec). Owned strings so the
    /// preview can outlive the tool-call stack frame.
    ApprovalNeeded {
        approval_id: String,
        tool_name: String,
        tool_call_id: String,
        risk: crate::tools::approval::ToolRisk,
        path: String,
        summary: String,
        diff: String,
    },
}

/// Throughput headroom as reported by the provider.
///
/// This answers "can I send right now", **not** "how much of my plan is left".
/// Those are different numbers and merging them produces a confident lie — see
/// `memory/decisions.md`. Account quota is provider-specific and must remain
/// separate from these short-window counters. Some adapters may attach an
/// independently reported account window, but a missing value must never be
/// filled with local usage.
///
/// `None` on a provider means it reports nothing, which is itself information the
/// ledger has to represent rather than silently treat as zero.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RateLimitSnapshot {
    pub requests_limit: Option<u64>,
    pub requests_remaining: Option<u64>,
    pub requests_reset: Option<String>,
    /// Generic token window fields used by OpenAI-compatible APIs. Anthropic
    /// exposes separate input/output fields below instead.
    #[serde(default)]
    pub tokens_limit: Option<u64>,
    #[serde(default)]
    pub tokens_remaining: Option<u64>,
    pub input_tokens_remaining: Option<u64>,
    pub output_tokens_remaining: Option<u64>,
    /// RFC 3339, stored raw. Nothing parses dates yet, so no date dependency.
    pub tokens_reset: Option<String>,
    pub retry_after_secs: Option<u64>,
    /// Optional account-window data emitted by a CLI provider, such as
    /// Claude Code's `rate_limit_event`. These are separate from API
    /// throughput counters above because they use a different wire shape.
    #[serde(default)]
    pub quota_window: Option<String>,
    #[serde(default)]
    pub quota_status: Option<String>,
    #[serde(default)]
    pub quota_used_percent: Option<f64>,
    #[serde(default)]
    pub quota_reset_at: Option<u64>,
    #[serde(default)]
    pub quota_overage_status: Option<String>,
    #[serde(default)]
    pub quota_overage_reset_at: Option<u64>,
    #[serde(default)]
    pub quota_is_using_overage: Option<bool>,
}

impl RateLimitSnapshot {
    /// True when the provider reported nothing at all.
    pub fn is_empty(&self) -> bool {
        self.requests_limit.is_none()
            && self.requests_remaining.is_none()
            && self.requests_reset.is_none()
            && self.tokens_limit.is_none()
            && self.tokens_remaining.is_none()
            && self.input_tokens_remaining.is_none()
            && self.output_tokens_remaining.is_none()
            && self.tokens_reset.is_none()
            && self.retry_after_secs.is_none()
            && self.quota_window.is_none()
            && self.quota_status.is_none()
            && self.quota_used_percent.is_none()
            && self.quota_reset_at.is_none()
            && self.quota_overage_status.is_none()
            && self.quota_overage_reset_at.is_none()
            && self.quota_is_using_overage.is_none()
    }
}

#[derive(Debug)]
pub struct Completion {
    /// The assistant turn's content blocks, in index order. Push this back into
    /// the message history verbatim.
    pub content: Vec<Value>,
    pub stop_reason: Option<String>,
    pub usage: Usage,
    /// Whether the endpoint actually reported token usage for this turn.
    pub usage_available: bool,
    pub limits: Option<RateLimitSnapshot>,
    /// The model the endpoint says actually served this turn.
    ///
    /// Distinct from the model that was *requested*, and the only trustworthy
    /// statement of which one ran: asking the model itself yields a guess, and a
    /// gateway is free to route a request anywhere. `None` means the endpoint
    /// did not say, which is not the same as agreeing.
    pub served_model: Option<String>,
    /// Updated provider-owned session reference. Persist only after the
    /// surrounding turn reaches a successful terminal state.
    pub provider_session: Option<ProviderSessionRef>,
}

/// Send the smallest possible real turn, to find out whether this provider can
/// actually serve one.
///
/// Presence of a credentials file is not the same as a working session: a
/// gateway can hold an account it has put into cooldown, or a key can be
/// revoked, and neither shows up on disk. The only honest way to say "signed
/// in" is to have been served.
///
/// Costs a few tokens, so this belongs on an explicit action — after a sign-in,
/// not on every render.
pub async fn probe(provider: &dyn Provider, model: &str) -> Result<()> {
    let request = TurnRequest {
        model: model.to_string(),
        system: None,
        messages: vec![Message::user_text("hi")],
        tools: Vec::new(),
        allow_tool_use: false,
        max_tokens: 1,
        effort: None,
        // Thinking would ignore max_tokens: 1 and make the cheapest possible
        // probe an expensive one.
        thinking: false,
        provider_session: None,
        interaction: None,
        cancel: None,
    };
    let mut sink = |_: StreamEvent<'_>| {};
    match provider.stream_turn(&request, &mut sink).await {
        Ok(_) => Ok(()),
        // `max_tokens` is the expected way for this to end: the turn was
        // served, which is the entire question being asked.
        Err(crate::error::HarnessError::StoppedEarly(_)) => Ok(()),
        Err(e) => Err(e),
    }
}

#[async_trait]
pub trait Provider: Send + Sync {
    /// Stable identifier used by configuration and the usage ledger.
    fn id(&self) -> &str;

    fn default_model(&self) -> &str;

    /// Models this provider accepts, with per-model effort allow-lists.
    ///
    /// Default: only [`Self::default_model`] with [`STANDARD_EFFORTS`].
    fn models(&self) -> Vec<ModelSpec> {
        catalogue(self.default_model(), &[], &[], EffortPolicy::Standard(&[]))
    }

    /// Picker / validation view of this provider.
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id().to_string(),
            default_model: self.default_model().to_string(),
            models: self.models(),
        }
    }

    /// Reject unknown model / effort pairs before a turn spends quota.
    fn validate_selection(&self, model: &str, effort: &str) -> std::result::Result<(), String> {
        validate_against(&self.models(), self.id(), model, effort)
    }

    /// Whether this provider can be used right now. Rendered by the launch
    /// picker and checked before starting a parent conversation here.
    fn auth_status(&self) -> AuthStatus;

    /// Whether this provider owns the model/tool loop itself, as Claude Code
    /// does through its authenticated CLI runtime. Such a provider receives
    /// the parent conversation but must not be given Zest's local tool loop or
    /// the explicit external-delegation tool.
    fn owns_agent_loop(&self) -> bool {
        false
    }

    /// Whether the endpoint honours Anthropic prompt caching (`cache_control`).
    ///
    /// Defaults to false, which is the honest answer for anything that is not
    /// Anthropic's own API. A gateway fronting a GPT or Gemini backend has no
    /// equivalent, and sending the field there is at best ignored and at worst
    /// a 400.
    fn supports_prompt_cache(&self) -> bool {
        false
    }

    /// Whether this provider can resume a stream after a process restart.
    fn resume_support(&self) -> ResumeSupport {
        ResumeSupport::Unsupported
    }

    /// Continue a provider-owned stream from a durable handle.
    ///
    /// The default keeps current providers safe and explicit: a provider must
    /// opt in only after its protocol can prove that the handle is sufficient.
    async fn resume_turn(
        &self,
        _req: &TurnRequest,
        _handle: &ResumeHandle,
        _on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    ) -> Result<Completion> {
        Err(crate::error::HarnessError::Other(format!(
            "provider `{}` does not support durable stream resume",
            self.id()
        )))
    }

    /// The callback must be `Send`: provider futures are `Send` so that delegated
    /// sub-agents can run concurrently on the tokio runtime.
    async fn stream_turn(
        &self,
        req: &TurnRequest,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    ) -> Result<Completion>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_models_list_accepts_only_default() {
        let cat = catalogue("gpt-5.6-sol", &[], &[], EffortPolicy::Standard(&[]));
        assert_eq!(cat.len(), 1);
        assert_eq!(cat[0].id, "gpt-5.6-sol");
        assert!(cat[0].efforts.contains(&"high".into()));
    }

    #[test]
    fn models_list_includes_default_if_missing() {
        let models = vec!["gpt-5.4".into()];
        let cat = catalogue(
            "gpt-5.6-sol",
            &models,
            &[],
            EffortPolicy::Standard(&["low".to_string()]),
        );
        assert_eq!(cat[0].id, "gpt-5.6-sol");
        assert_eq!(cat[1].id, "gpt-5.4");
        assert_eq!(cat[0].efforts, vec!["low".to_string()]);
    }

    #[test]
    fn codex_builtin_catalogue_includes_luna() {
        let cat = catalogue(
            "gpt-5.6-sol",
            &[],
            CODEX_KNOWN_MODELS,
            EffortPolicy::Standard(&[]),
        );
        assert!(cat.iter().any(|m| m.id == "gpt-5.6-luna"));
        assert!(cat.iter().any(|m| m.id == "gpt-5.6-terra"));
        assert!(cat.iter().any(|m| m.id == "gpt-5.6-sol"));
    }

    #[test]
    fn other_gateway_empty_models_stays_default_only() {
        let cat = catalogue("gpt-5.6-sol", &[], &[], EffortPolicy::Standard(&[]));
        assert_eq!(cat.len(), 1);
        assert_eq!(cat[0].id, "gpt-5.6-sol");
    }

    #[test]
    fn openai_compatible_catalogue_does_not_advertise_effort_controls() {
        let cat = catalogue(
            "deepseek-v4-flash",
            &["deepseek-v4-flash".into(), "deepseek-v4-pro".into()],
            &[],
            EffortPolicy::Unsupported,
        );
        assert_eq!(cat.len(), 2);
        assert!(cat.iter().all(|model| model.efforts.is_empty()));
    }

    /// The picker and the live provider must agree about what is selectable.
    ///
    /// Two independent matches over the same config had drifted:
    /// `descriptor_from_config` built the catalogue from the configured model
    /// alone, while `AnthropicProvider::native(..).with_default_model(..)`
    /// *prepended* it to a catalogue that already held `DEFAULT_MODEL`. A user
    /// with `model = "claude-haiku-5"` saw one model in the picker and could
    /// select two at runtime.
    #[test]
    fn the_picker_catalogue_matches_the_live_provider_catalogue() {
        std::env::set_var("ZEST_TEST_DRIFT_KEY", "present");
        let config = crate::config::Config::parse(
            r#"
[providers.house]
kind = "anthropic"
api_key_env = "ZEST_TEST_DRIFT_KEY"
model = "claude-haiku-5"
"#,
        )
        .expect("valid");
        let entry = &config.providers["house"];

        let (registry, skipped) = crate::provider::registry::ProviderRegistry::from_config(&config);
        assert!(skipped.is_empty(), "{skipped:?}");
        let live = registry.get("house").expect("built").descriptor();
        let picker = descriptor_from_config("house", entry);
        std::env::remove_var("ZEST_TEST_DRIFT_KEY");

        assert_eq!(picker.default_model, live.default_model);
        let picker_ids: Vec<&str> = picker.models.iter().map(|m| m.id.as_str()).collect();
        let live_ids: Vec<&str> = live.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            picker_ids, live_ids,
            "the picker must offer exactly what the provider accepts"
        );
    }

    #[test]
    fn durable_resume_defaults_to_unsupported_and_handles_are_serializable() {
        assert_eq!(ResumeSupport::default(), ResumeSupport::Unsupported);

        let handle = ResumeHandle::new("provider-run-1").with_cursor("event-9");
        let encoded = serde_json::to_value(&handle).unwrap();
        assert_eq!(encoded["providerRunId"], "provider-run-1");
        assert_eq!(encoded["cursor"], "event-9");
        assert_eq!(
            serde_json::from_value::<ResumeHandle>(encoded).unwrap(),
            handle
        );
    }
}
