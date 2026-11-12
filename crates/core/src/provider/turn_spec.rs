//! The canonical description of one agent turn, with no CLI in it.
//!
//! Every provider that drives an external coding agent needs the same four
//! things decided before it can build a command: what the agent is, which
//! instructions it carries, which tools it may reach, and which channel those
//! instructions travel on. Each provider used to answer all four inline, so
//! `claude_code`, `cursor_acp`, and `codex_app_server` grew three unrelated
//! conversation renderers and no shared vocabulary at all.
//!
//! This module is that vocabulary. It is ported from `anyagentcli-mcp`'s
//! `prompt-composition/` layer, which solved the same problem for five CLIs.
//! What is *not* ported is its `ask`/`build` profile axis: that models a config
//! concept anygent has and Zest does not. Zest expresses read-only through the
//! provider's own permission mode, so inventing a second switch here would let
//! the two disagree.
//!
//! Nothing in this file knows a flag name. Translation to argv is the
//! provider's job, and lives next to the provider that owns the wire format.

use serde_json::Value;

use crate::anthropic::types::Message;

/// Where an agent's instructions travel.
///
/// `InputAugmented` folds them into the prompt the agent reads as user text.
/// `NativeDelivery` hands them to a channel the CLI owns, which keeps them out
/// of the per-turn payload entirely once a session is being resumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositionPolicy {
    InputAugmented,
    NativeDelivery,
}

/// Which native channel carries instructions under [`CompositionPolicy::NativeDelivery`].
///
/// `Input` is the degenerate case and only appears under `InputAugmented`; it is
/// named so a trace can state the channel unconditionally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMode {
    Input,
    /// A dedicated system-prompt flag, such as Claude Code's
    /// `--append-system-prompt`.
    SystemPromptFlag,
    /// A generated agent-definition file the CLI loads by name.
    AgentFile,
}

/// Whether this agent is the conversation the user is having, or a worker
/// something else delegated to.
///
/// The distinction is not cosmetic: it selects the safeguard text. A worker is
/// told it reports to an orchestrator and may not push, install, or delete. A
/// parent is told none of that, because it *is* the user's session and those
/// claims would be false.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRole {
    Parent,
    Worker,
}

/// The set of tools an agent may reach, as tool names with no flag syntax.
///
/// `allow` is the scope boundary: the tools that make sense inside Zest.
/// `deny` is a user-configured escape hatch layered on top, and is empty by
/// default. Deliberately *not* an auto-approve list: a provider that gates tool
/// calls behind an approval card must keep that card as the only gate, and an
/// auto-approve list would silently route around it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolScope {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
}

impl ToolScope {
    pub fn new(allow: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            allow: allow.into_iter().map(Into::into).collect(),
            deny: Vec::new(),
        }
    }

    pub fn with_deny(mut self, deny: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.deny = deny.into_iter().map(Into::into).collect();
        self
    }
}

/// One addressable agent.
///
/// Today there is exactly one per turn and it is named `parent`. The struct
/// exists in this shape so that named agents — the thing anygent's `agents` map
/// expresses — can be added without reshaping every caller.
#[derive(Debug, Clone)]
pub struct AgentProfile {
    pub name: String,
    pub role: AgentRole,
    /// The agent's own instructions, before any provider base prompt.
    pub instructions: Option<String>,
    pub tools: ToolScope,
}

impl AgentProfile {
    /// The single agent behind the user's own conversation.
    pub fn parent(tools: ToolScope) -> Self {
        Self {
            name: "parent".into(),
            role: AgentRole::Parent,
            instructions: None,
            tools,
        }
    }

    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        let instructions = instructions.into();
        self.instructions = (!instructions.trim().is_empty()).then_some(instructions);
        self
    }
}

/// What a provider can actually do with a composed turn.
///
/// A provider declares this once; [`compose`] uses it to decide the channel
/// rather than having each provider re-derive the same answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompositionCapabilities {
    pub default_policy: CompositionPolicy,
    pub supports_native_delivery: bool,
    pub native_delivery_mode: DeliveryMode,
    /// Whether the CLI keeps its own conversation across turns. When true and a
    /// session is being resumed, only the newest user message is sent.
    pub owns_conversation: bool,
}

impl CompositionCapabilities {
    /// A provider with no native channel: everything rides in the prompt.
    pub const INPUT_ONLY: Self = Self {
        default_policy: CompositionPolicy::InputAugmented,
        supports_native_delivery: false,
        native_delivery_mode: DeliveryMode::Input,
        owns_conversation: false,
    };

    /// A CLI that keeps the conversation between turns, with instructions
    /// delivered as the opening message rather than through a flag.
    ///
    /// Claude Code has an `--append-system-prompt`, and this deliberately does
    /// not use it: on Windows the whole command line is capped near 32 KB, and
    /// a system prompt carrying project docs and skill descriptions can pass
    /// that on its own. Sent as the first message it goes over stdin, which has
    /// no such ceiling, and the resumed session carries it from then on.
    pub const SESSION_OWNING: Self = Self {
        default_policy: CompositionPolicy::InputAugmented,
        supports_native_delivery: false,
        native_delivery_mode: DeliveryMode::Input,
        owns_conversation: true,
    };
}

/// One decision recorded while composing, for logs.
///
/// Composition has three inputs that can each override the next, and when the
/// result is wrong the useful question is *which* input decided. Carrying the
/// answer costs a few allocations per turn and removes the guesswork.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceEntry {
    pub step: &'static str,
    pub value: String,
    pub reason: &'static str,
}

impl TraceEntry {
    fn new(step: &'static str, value: impl Into<String>, reason: &'static str) -> Self {
        Self {
            step,
            value: value.into(),
            reason,
        }
    }
}

/// One turn, described without reference to any CLI's wire format.
pub struct TurnSpec<'a> {
    pub profile: &'a AgentProfile,
    /// Zest's operating context for this session.
    pub system: Option<&'a str>,
    pub messages: &'a [Message],
    pub model: &'a str,
    pub effort: Option<&'a str>,
    /// Whether the provider is continuing a conversation it already holds.
    /// Only meaningful when [`CompositionCapabilities::owns_conversation`].
    pub resumed: bool,
    /// Provider-side override of the composition policy. `None` takes the
    /// provider's declared default.
    pub policy_override: Option<CompositionPolicy>,
}

/// The composed turn, ready for a provider to place on its own channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedTurn {
    /// Instructions for the provider's native system channel. `Some` only under
    /// [`CompositionPolicy::NativeDelivery`].
    pub system: Option<String>,
    /// What the agent reads as the user turn.
    pub prompt: String,
    pub policy: CompositionPolicy,
    pub delivery: DeliveryMode,
    pub trace: Vec<TraceEntry>,
}

/// Resolve the policy for a turn, recording why.
///
/// Precedence mirrors anygent's `resolve-policy.ts`: an explicit override wins,
/// otherwise the provider's declared default. A request for native delivery
/// from a provider that has no native channel is a caller bug, not something to
/// paper over, so it falls back and says so in the trace rather than throwing —
/// a turn that still runs with the instructions in the prompt is strictly
/// better than a turn that does not run.
pub fn resolve_policy(
    spec: &TurnSpec<'_>,
    capabilities: &CompositionCapabilities,
) -> (CompositionPolicy, Vec<TraceEntry>) {
    let mut trace = Vec::new();
    let requested = match spec.policy_override {
        Some(policy) => {
            trace.push(TraceEntry::new(
                "policy",
                policy.label(),
                "explicit override",
            ));
            policy
        }
        None => {
            let policy = capabilities.default_policy;
            trace.push(TraceEntry::new(
                "policy",
                policy.label(),
                "provider default",
            ));
            policy
        }
    };

    if requested == CompositionPolicy::NativeDelivery && !capabilities.supports_native_delivery {
        trace.push(TraceEntry::new(
            "policy",
            CompositionPolicy::InputAugmented.label(),
            "provider declares no native channel",
        ));
        return (CompositionPolicy::InputAugmented, trace);
    }
    (requested, trace)
}

impl CompositionPolicy {
    fn label(self) -> &'static str {
        match self {
            Self::InputAugmented => "input-augmented",
            Self::NativeDelivery => "native-delivery",
        }
    }
}

impl DeliveryMode {
    fn label(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::SystemPromptFlag => "system-prompt-flag",
            Self::AgentFile => "agent-file",
        }
    }
}

/// Compose a turn into the pieces a provider places on its channels.
///
/// Under `InputAugmented` the instructions are prefixed to the prompt, which is
/// the only option for a provider with no system channel. Under
/// `NativeDelivery` they come back separately, and — crucially — the prompt
/// shrinks to just the new message once the CLI is holding the conversation
/// itself. That second effect is where the cost goes: re-serialising a whole
/// transcript on every turn is what the native channel exists to avoid.
pub fn compose(spec: &TurnSpec<'_>, capabilities: &CompositionCapabilities) -> ComposedTurn {
    let (policy, mut trace) = resolve_policy(spec, capabilities);

    let delivery = match policy {
        CompositionPolicy::InputAugmented => DeliveryMode::Input,
        CompositionPolicy::NativeDelivery => capabilities.native_delivery_mode,
    };
    trace.push(TraceEntry::new(
        "delivery",
        delivery.label(),
        "resolved policy",
    ));
    trace.push(TraceEntry::new(
        "agent",
        spec.profile.name.clone(),
        "profile composed for",
    ));

    // Only skip the transcript when the provider is genuinely holding it. A
    // provider that owns a conversation but is starting a fresh one still needs
    // every prior message, or the continuation loses its context.
    let carry_history = !(capabilities.owns_conversation && spec.resumed);
    trace.push(TraceEntry::new(
        "history",
        if carry_history { "full" } else { "latest-only" },
        if carry_history {
            "provider does not hold this conversation"
        } else {
            "provider is resuming its own session"
        },
    ));

    let body = if carry_history {
        let mut body = render_conversation(spec.messages);
        // A flattened transcript has no turn boundary in it, so an agent handed
        // one has to be told which part is the request. Only worth saying when
        // there is more than one message to choose between.
        if spec.messages.len() > 1 {
            body.push_str(CONTINUE_INSTRUCTION);
        }
        body
    } else {
        latest_user_message(spec.messages)
    };

    // Under `InputAugmented` the instructions *are* conversation content, so a
    // provider still holding that conversation already has them from the turn
    // that opened it. Re-sending them would prepend the whole system prompt to
    // every message for the life of the session, which is the cost the resume
    // exists to avoid. A native channel is re-established per process launch,
    // so it always carries them.
    let carry_instructions = carry_history || policy == CompositionPolicy::NativeDelivery;
    trace.push(TraceEntry::new(
        "instructions",
        if carry_instructions { "sent" } else { "held" },
        if carry_instructions {
            "channel does not persist across turns"
        } else {
            "already in the session the provider is resuming"
        },
    ));
    let instructions = carry_instructions
        .then(|| build_instructions(spec))
        .flatten();

    match policy {
        CompositionPolicy::NativeDelivery => ComposedTurn {
            system: instructions,
            prompt: body,
            policy,
            delivery,
            trace,
        },
        CompositionPolicy::InputAugmented => {
            let prompt = match instructions {
                Some(instructions) if !body.is_empty() => format!("{instructions}\n\n{body}"),
                Some(instructions) => instructions,
                None => body,
            };
            ComposedTurn {
                system: None,
                prompt,
                policy,
                delivery,
                trace,
            }
        }
    }
}

/// Safeguards, then Zest's operating context, then the agent's own instructions.
fn build_instructions(spec: &TurnSpec<'_>) -> Option<String> {
    let mut parts: Vec<&str> = vec![safeguards(spec.profile.role)];
    if let Some(system) = spec.system.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(system);
    }
    if let Some(agent) = spec
        .profile
        .instructions
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        parts.push(agent);
    }
    let joined = parts
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!joined.is_empty()).then_some(joined)
}

/// Role-specific instructions layered ahead of the session context.
///
/// A parent gets nothing here, and that is the whole point. Its guidance is
/// already composed into the session system prompt by `runtime`, which swaps in
/// [`crate::prompt::CLAUDE_CODE_PARENT_SYSTEM`] for providers that own their
/// agent loop. Returning that constant again would put it in the turn twice.
///
/// The worker text is ported from anygent's `safeguards.ts`, whose framing — an
/// autonomous agent reporting to an orchestrator, forbidden from pushing or
/// installing — is true of a delegated worker and false of the user's own
/// session. Applied to a parent it would have the agent refuse work the user
/// directly asked for.
pub fn safeguards(role: AgentRole) -> &'static str {
    match role {
        AgentRole::Parent => "",
        AgentRole::Worker => WORKER_SAFEGUARDS,
    }
}

/// Reserved for delegated workers. No live caller yet: Zest's external workers
/// still run through `delegate_external`, which has its own prompt.
const WORKER_SAFEGUARDS: &str = "\
You are an autonomous coding agent invoked by an orchestrator, not by a human. \
Your answer is read by the orchestrator, so be precise and technical.

These actions are outside your boundary. If asked for one, say you cannot \
perform it and tell the orchestrator to run it under human supervision:

1. Deleting files, directories, or database records. If cleanup is needed, list \
the exact targets with justification instead of acting.
2. Moving, renaming, or overwriting files outside your working directory.
3. Migrations, schema changes, or seed scripts against a shared database. \
Surface the exact command instead.
4. `git push`, opening pull requests, or merging branches.
5. Installing dependencies that are not already in the manifest.";

const CONTINUE_INSTRUCTION: &str =
    "\nContinue from the conversation above and complete the latest \
user request. Report the result clearly when the work is finished.";

/// Render a conversation as plain text, for a CLI that takes one prompt.
///
/// This is the single implementation. `claude_code`, `cursor_acp`, and
/// `codex_app_server` each grew their own, and they disagreed about the two
/// cases that matter: `cursor_acp` dropped tool calls and results entirely, and
/// `claude_code` rendered a tool call as a bare `[Zest tool call: name]` with
/// no input. A resumed conversation rendered by either loses the reasoning
/// behind its own prior edits.
pub fn render_conversation(messages: &[Message]) -> String {
    let mut output = String::new();
    for message in messages {
        let role = match message.role.as_str() {
            "assistant" => "Assistant",
            "user" => "User",
            other => other,
        };
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(role);
        output.push_str(":\n");
        let body = render_content(&message.content);
        if body.is_empty() {
            output.push_str("[non-text content]\n");
        } else {
            output.push_str(&body);
            output.push('\n');
        }
    }
    output
}

/// The newest user message, for a provider already holding the history.
pub fn latest_user_message(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| render_content(&message.content))
        .unwrap_or_default()
}

/// One message's content blocks as text, including tool activity.
pub fn render_content(content: &[Value]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for block in content {
        if let Some(rendered) = render_block(block) {
            if !rendered.is_empty() {
                parts.push(rendered);
            }
        }
    }
    parts.join("\n")
}

fn render_block(block: &Value) -> Option<String> {
    match block.get("type").and_then(Value::as_str) {
        Some("tool_use") => {
            let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
            let input = block
                .get("input")
                .map(compact_json)
                .unwrap_or_else(|| "{}".into());
            Some(format!("[Tool call: {name}]\nInput: {input}"))
        }
        Some("tool_result") => {
            let label = if block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                " (error)"
            } else {
                ""
            };
            let body = block
                .get("content")
                .and_then(text_value)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "[empty]".into());
            Some(format!("[Tool result{label}]\n{body}"))
        }
        _ => text_value(block),
    }
}

/// Best-effort text out of a content block of unknown shape.
pub fn text_value(value: &Value) -> Option<String> {
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    if let Some(items) = value.as_array() {
        let joined = items
            .iter()
            .filter_map(text_value)
            .collect::<Vec<_>>()
            .join("\n");
        return (!joined.is_empty()).then_some(joined);
    }
    value
        .get("content")
        .and_then(text_value)
        .filter(|text| !text.is_empty())
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scope() -> ToolScope {
        ToolScope::new(["Read", "Write"])
    }

    fn conversation() -> Vec<Message> {
        vec![
            Message::user_text("Inspect the loader."),
            Message::assistant(vec![json!({"type": "text", "text": "I will inspect it."})]),
            Message::user_text("Now implement the fix."),
        ]
    }

    fn spec<'a>(profile: &'a AgentProfile, messages: &'a [Message], resumed: bool) -> TurnSpec<'a> {
        TurnSpec {
            profile,
            system: Some("Follow the project rules."),
            messages,
            model: "sonnet",
            effort: None,
            resumed,
            policy_override: None,
        }
    }

    const NATIVE: CompositionCapabilities = CompositionCapabilities {
        default_policy: CompositionPolicy::NativeDelivery,
        supports_native_delivery: true,
        native_delivery_mode: DeliveryMode::SystemPromptFlag,
        owns_conversation: true,
    };

    #[test]
    fn input_augmented_puts_everything_in_the_prompt() {
        let profile = AgentProfile::parent(scope());
        let messages = conversation();
        let composed = compose(
            &spec(&profile, &messages, false),
            &CompositionCapabilities::INPUT_ONLY,
        );

        assert_eq!(composed.system, None);
        assert_eq!(composed.policy, CompositionPolicy::InputAugmented);
        assert_eq!(composed.delivery, DeliveryMode::Input);
        assert!(composed.prompt.contains("Follow the project rules."));
        assert!(composed.prompt.contains("Inspect the loader."));
        assert!(composed.prompt.contains("Now implement the fix."));
    }

    #[test]
    fn native_delivery_separates_instructions_from_the_prompt() {
        let profile = AgentProfile::parent(scope());
        let messages = conversation();
        let composed = compose(&spec(&profile, &messages, false), &NATIVE);

        let system = composed.system.as_deref().expect("native system channel");
        assert!(system.contains("Follow the project rules."));
        assert!(!composed.prompt.contains("Follow the project rules."));
        assert_eq!(composed.delivery, DeliveryMode::SystemPromptFlag);
        // Not resuming yet, so the transcript still travels.
        assert!(composed.prompt.contains("Inspect the loader."));
    }

    /// The point of a provider-held session: the prompt stops carrying history.
    #[test]
    fn resuming_a_provider_session_sends_only_the_newest_message() {
        let profile = AgentProfile::parent(scope());
        let messages = conversation();
        let composed = compose(&spec(&profile, &messages, true), &NATIVE);

        assert_eq!(composed.prompt, "Now implement the fix.");
        assert!(!composed.prompt.contains("Inspect the loader."));
    }

    /// The cost this exists to avoid: under `InputAugmented` the instructions
    /// are conversation content, so repeating them would prepend the whole
    /// system prompt to every message for the life of the session.
    #[test]
    fn a_resumed_session_does_not_repeat_the_instructions() {
        let profile = AgentProfile::parent(scope()).with_instructions("Prefer small diffs.");
        let messages = conversation();

        let opening = compose(
            &spec(&profile, &messages, false),
            &CompositionCapabilities::SESSION_OWNING,
        );
        assert!(opening.prompt.contains("Follow the project rules."));
        assert!(opening.prompt.contains("Prefer small diffs."));

        let resumed = compose(
            &spec(&profile, &messages, true),
            &CompositionCapabilities::SESSION_OWNING,
        );
        assert_eq!(resumed.prompt, "Now implement the fix.");
        assert!(resumed
            .trace
            .iter()
            .any(|entry| entry.step == "instructions" && entry.value == "held"));
    }

    /// A native channel is a flag on a fresh process, so it has to be set every
    /// launch even when the conversation itself is being resumed.
    #[test]
    fn a_native_channel_repeats_the_instructions_every_launch() {
        let profile = AgentProfile::parent(scope());
        let messages = conversation();
        let composed = compose(&spec(&profile, &messages, true), &NATIVE);

        let system = composed.system.expect("native system channel");
        assert!(system.contains("Follow the project rules."));
    }

    /// A provider that owns no conversation must never drop history, even if a
    /// caller sets `resumed` by mistake.
    #[test]
    fn resume_without_provider_ownership_keeps_the_transcript() {
        let profile = AgentProfile::parent(scope());
        let messages = conversation();
        let composed = compose(
            &spec(&profile, &messages, true),
            &CompositionCapabilities::INPUT_ONLY,
        );

        assert!(composed.prompt.contains("Inspect the loader."));
    }

    #[test]
    fn native_delivery_falls_back_when_the_provider_has_no_channel() {
        let profile = AgentProfile::parent(scope());
        let messages = conversation();
        let mut spec = spec(&profile, &messages, false);
        spec.policy_override = Some(CompositionPolicy::NativeDelivery);

        let composed = compose(&spec, &CompositionCapabilities::INPUT_ONLY);
        assert_eq!(composed.policy, CompositionPolicy::InputAugmented);
        assert!(composed
            .trace
            .iter()
            .any(|entry| entry.reason == "provider declares no native channel"));
    }

    #[test]
    fn agent_instructions_follow_the_session_context() {
        let profile = AgentProfile::parent(scope()).with_instructions("Prefer small diffs.");
        let messages = conversation();
        let composed = compose(&spec(&profile, &messages, false), &NATIVE);

        let system = composed.system.expect("native system channel");
        let rules = system.find("Follow the project rules.").expect("session");
        let agent = system.find("Prefer small diffs.").expect("agent");
        assert!(rules < agent, "agent instructions come last:\n{system}");
    }

    /// `runtime` already composes the parent guidance into the session prompt.
    /// Adding it again here would send it twice, and applying the *worker* text
    /// to a parent would have it refuse work the user directly asked for.
    #[test]
    fn a_parent_never_receives_worker_safeguards() {
        assert_eq!(safeguards(AgentRole::Parent), "");
        assert!(safeguards(AgentRole::Worker).contains("orchestrator"));

        let profile = AgentProfile::parent(scope());
        let messages = conversation();
        let composed = compose(&spec(&profile, &messages, false), &NATIVE);
        let system = composed.system.expect("native system channel");
        assert!(!system.contains("orchestrator"), "{system}");
        assert!(system.starts_with("Follow the project rules."), "{system}");
    }

    /// The divergence this module exists to remove: two of the three previous
    /// renderers dropped tool activity, so a resumed conversation could not see
    /// why its own earlier edits happened.
    #[test]
    fn tool_calls_and_results_survive_rendering() {
        let messages = vec![
            Message::assistant(vec![json!({
                "type": "tool_use",
                "name": "edit_file",
                "input": {"path": "src/lib.rs"},
            })]),
            Message::user_blocks(vec![json!({
                "type": "tool_result",
                "content": "applied",
            })]),
            Message::user_blocks(vec![json!({
                "type": "tool_result",
                "is_error": true,
                "content": "no such file",
            })]),
        ];

        let rendered = render_conversation(&messages);
        assert!(rendered.contains("[Tool call: edit_file]"));
        assert!(rendered.contains("src/lib.rs"));
        assert!(rendered.contains("[Tool result]\napplied"));
        assert!(rendered.contains("[Tool result (error)]\nno such file"));
    }

    #[test]
    fn a_message_with_no_text_is_labelled_rather_than_dropped() {
        let messages = vec![Message::user_blocks(vec![json!({"type": "image"})])];
        assert!(render_conversation(&messages).contains("[non-text content]"));
    }

    #[test]
    fn tool_scope_carries_a_user_deny_list() {
        let scope = ToolScope::new(["Read", "Write"]).with_deny(["Write"]);
        assert_eq!(scope.allow, vec!["Read".to_string(), "Write".to_string()]);
        assert_eq!(scope.deny, vec!["Write".to_string()]);
    }
}
