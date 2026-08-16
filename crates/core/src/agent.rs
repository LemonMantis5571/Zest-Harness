//! The agent loop.
//!
//! Request the model, execute whatever tools it asks for, feed the results back,
//! repeat until it stops asking. Everything interesting about a harness lives
//! either side of this file — the provider layer above it, the tool layer and the
//! permission model below — but this is the spine.
//!
//! The loop is provider-agnostic. It describes the turn it wants and lets the
//! provider decide how to express that on the wire.
//!
//! Provider-facing history is **transactional**: mutations are staged and only
//! committed when the turn reaches a complete terminal state. Errors and
//! cancellation leave `Agent::messages` unchanged so wire history never contains
//! a half-built assistant/tool turn. UI transcript is the front-end's job.
//!
//! Sensitive tool results are redacted when committed to durable wire history
//! while the live in-memory turn still sees the real body for the model.

use std::sync::{Arc, Mutex};

use crate::anthropic::types::{tool_result, tool_uses, Message, Usage};
use crate::cancel::{wait_cancel, CancelToken};
use crate::error::{HarnessError, Result};
use crate::provider::{
    Provider, ProviderInteractionHost, ProviderSessionRef, StreamEvent, SystemPrompt, TurnRequest,
};
use crate::thread::new_id;
use crate::tools::approval::{
    ApprovalDecision, ApprovalPolicy, ApprovalRequest, Approver, DenyApprover, PolicyOutcome,
    ToolRisk,
};
use crate::tools::prepared::PreparedToolCall;
use crate::tools::question::{parse_question_input, DenyQuestioner, Questioner, ASK_USER_TOOL};
use crate::tools::ToolRegistry;
use crate::usage::Ledger;

const REDACTED_SENSITIVE_RESULT: &str =
    "[redacted: sensitive tool result omitted from persisted history]";

pub struct Agent {
    provider: Arc<dyn Provider>,
    tools: ToolRegistry,
    /// Shared so delegated sub-agents on other providers bill into the same book.
    ledger: Option<Arc<Mutex<Ledger>>>,
    /// Gate for write/exec tools. Defaults to deny-all when unset.
    approver: Arc<dyn Approver>,
    /// Mode + session grants, consulted before the approver is ever called.
    /// Shared so a front-end can flip the mode mid-session.
    policy: Arc<Mutex<ApprovalPolicy>>,
    /// Front-end hook for the provider-independent `ask_user` tool.
    questioner: Arc<dyn Questioner>,
    pub model: String,
    /// Budgets reasoning *and* text together on providers that think. Streaming
    /// means there is no HTTP timeout pressure, so this is a ceiling rather than
    /// a target.
    pub max_tokens: u32,
    /// A request, not a command — providers that have no notion of effort ignore it.
    pub effort: String,
    pub system: Option<SystemPrompt>,
    pub messages: Vec<Message>,
    /// Provider-native continuation state. The desktop copies this to the
    /// durable thread only after a successful terminal turn.
    pub provider_session: Option<ProviderSessionRef>,
    pub provider_interaction: Option<Arc<dyn ProviderInteractionHost>>,
    /// Last completed turn's usage (input fills the context window estimate).
    pub last_usage: Option<Usage>,
    /// Tool-use ids whose results must be redacted when persisting wire history.
    sensitive_tool_ids: Vec<String>,
}

impl Agent {
    pub fn new(provider: Arc<dyn Provider>, tools: ToolRegistry) -> Self {
        let model = provider.default_model().to_string();
        Self {
            provider,
            tools,
            ledger: None,
            approver: Arc::new(DenyApprover),
            policy: Arc::new(Mutex::new(ApprovalPolicy::default())),
            questioner: Arc::new(DenyQuestioner),
            model,
            max_tokens: 32_000,
            effort: "high".to_string(),
            system: None,
            messages: Vec::new(),
            provider_session: None,
            provider_interaction: None,
            last_usage: None,
            sensitive_tool_ids: Vec::new(),
        }
    }

    /// Accepts a bare string for the common case where the whole prompt is
    /// stable, or a [`SystemPrompt`] when part of it describes the environment
    /// and must sit outside the cache breakpoint.
    pub fn with_system(mut self, system: impl Into<SystemPrompt>) -> Self {
        self.system = Some(system.into());
        self
    }

    /// The system prompt as the model reads it, both halves in order.
    pub fn system_text(&self) -> String {
        self.system
            .as_ref()
            .map(SystemPrompt::text)
            .unwrap_or_default()
    }

    pub fn with_ledger(mut self, ledger: Arc<Mutex<Ledger>>) -> Self {
        self.ledger = Some(ledger);
        self
    }

    /// Restore prior turns so the model sees conversation history after reopen.
    pub fn with_messages(mut self, messages: Vec<Message>) -> Self {
        self.messages = messages;
        self
    }

    pub fn with_provider_session(mut self, session: Option<ProviderSessionRef>) -> Self {
        self.provider_session = session;
        self
    }

    pub fn with_provider_interaction(
        mut self,
        interaction: Arc<dyn ProviderInteractionHost>,
    ) -> Self {
        self.provider_interaction = Some(interaction);
        self
    }

    pub fn provider_session(&self) -> Option<ProviderSessionRef> {
        self.provider_session.clone()
    }

    pub fn clear_provider_session(&mut self) {
        self.provider_session = None;
    }

    /// Hook for desktop (or a CLI prompt) to allow/deny gated tools.
    pub fn with_approver(mut self, approver: Arc<dyn Approver>) -> Self {
        self.approver = approver;
        self
    }

    /// Share the permission policy so a front-end can change mode mid-session.
    pub fn with_policy(mut self, policy: Arc<Mutex<ApprovalPolicy>>) -> Self {
        self.policy = policy;
        self
    }

    /// Hook for desktop or another interactive front-end to answer
    /// provider-neutral `ask_user` calls.
    pub fn with_questioner(mut self, questioner: Arc<dyn Questioner>) -> Self {
        self.questioner = questioner;
        self
    }

    pub fn policy(&self) -> Arc<Mutex<ApprovalPolicy>> {
        self.policy.clone()
    }

    pub fn clear_messages(&mut self) {
        self.messages.clear();
        self.sensitive_tool_ids.clear();
        self.provider_session = None;
    }

    /// Which provider this agent spends against. Keyed on by the usage ledger.
    pub fn provider_id(&self) -> &str {
        self.provider.id()
    }

    /// Shared provider handle for auxiliary display-only operations such as
    /// reading-diff generation. These operations never mutate agent history.
    pub fn provider(&self) -> Arc<dyn Provider> {
        self.provider.clone()
    }

    /// Registered tool names (stable order). Used to assert worker-tool wiring.
    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.names()
    }

    /// Validate a model/effort pair against this agent's provider catalogue.
    pub fn validate_options(&self, model: &str, effort: &str) -> std::result::Result<(), String> {
        self.provider.validate_selection(model, effort)
    }

    /// Provider catalogue for pickers.
    pub fn descriptor(&self) -> crate::provider::ProviderDescriptor {
        self.provider.descriptor()
    }

    /// Wire history safe for durable persistence (sensitive tool bodies redacted).
    /// Live [`Self::messages`] keeps the real bodies for the in-session model.
    pub fn messages_for_persist(&self) -> Vec<Message> {
        redact_sensitive_staged(self.messages.clone(), &self.sensitive_tool_ids)
    }

    /// Replace a long wire history with a provider-generated checkpoint.
    ///
    /// The summarization request receives the persistence-safe history, so a
    /// sensitive tool result cannot be copied into the durable checkpoint. It
    /// calls no tools and asks for no thinking: compaction is maintenance, not
    /// a second agent turn, and it must remain cheap across providers. The tool
    /// list is still declared where that keeps the cached prefix intact — see
    /// `allow_tool_use`, which is what actually forbids the call.
    pub async fn compact_context(&mut self) -> Result<String> {
        if self.messages.len() < 4 {
            return Err(HarnessError::Other(
                "there is not enough conversation to compact yet".into(),
            ));
        }

        let mut messages = self.messages_for_persist();
        messages.push(Message::user_text(
            "Create a concise context checkpoint for the next coding turn. ".to_string()
                + "Preserve the user’s goals, decisions, constraints, files changed, "
                + "unfinished work, and important tool findings. Do not invent facts. "
                + "Return only the checkpoint, with compact headings.",
        ));
        let request = TurnRequest {
            model: self.model.clone(),
            system: self.system.clone(),
            messages,
            // The tool list still goes on the wire, and `allow_tool_use` keeps
            // the model from touching it. Dropping the tools would make this
            // prompt diverge from a normal turn at byte zero, so compaction
            // would both miss the session's cached prefix and write a second
            // full copy of it — twice the price of the thing it exists to
            // make cheaper.
            tools: self.tools_for_model(),
            allow_tool_use: false,
            max_tokens: 4_096,
            effort: None,
            thinking: false,
            provider_session: self.provider_session.clone(),
            interaction: self.provider_interaction.clone(),
            cancel: None,
        };
        let mut sink = |_event: StreamEvent<'_>| {};
        let completion = match self.provider.stream_turn(&request, &mut sink).await {
            Ok(completion) => completion,
            Err(error) => {
                self.provider_session = None;
                return Err(error);
            }
        };
        self.provider_session = None;
        if let Some(ledger) = &self.ledger {
            if let Ok(mut ledger) = ledger.lock() {
                ledger.record(
                    self.provider.id(),
                    billed_model(&request.model, completion.served_model.as_deref()),
                    &completion,
                );
            }
        }

        let summary = completion
            .content
            .iter()
            .filter_map(|block| {
                (block.get("type").and_then(|kind| kind.as_str()) == Some("text"))
                    .then(|| block.get("text").and_then(|text| text.as_str()))
                    .flatten()
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();
        if summary.is_empty() {
            return Err(HarnessError::Other(
                "provider returned an empty context checkpoint".into(),
            ));
        }

        self.messages = vec![
            Message::user_text(
                "[Zest context checkpoint] The earlier conversation is represented by the "
                    .to_string()
                    + "assistant checkpoint that follows. Use it as working context.",
            ),
            Message::assistant(vec![serde_json::json!({
                "type": "text",
                "text": summary,
            })]),
        ];
        self.sensitive_tool_ids.clear();
        self.provider_session = None;
        // The compaction request measured the old history, not the compacted
        // conversation. Do not let that maintenance request masquerade as the
        // next turn's context usage in the footer.
        self.last_usage = None;
        Ok(summary)
    }

    /// Send one user message and run to completion, executing tools as asked.
    ///
    /// Wire history is committed only after a complete terminal turn. Pass
    /// `cancel` to cooperatively abort between provider/tool steps.
    pub async fn send(
        &mut self,
        user_input: &str,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    ) -> Result<()> {
        self.send_cancellable(user_input, on_event, None).await
    }

    pub async fn send_cancellable(
        &mut self,
        user_input: &str,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
        cancel: Option<&CancelToken>,
    ) -> Result<()> {
        self.send_user_cancellable(Message::user_text(user_input), on_event, cancel)
            .await
    }

    /// Multimodal / structured user turn (text + image blocks, etc.).
    pub async fn send_blocks_cancellable(
        &mut self,
        content: Vec<serde_json::Value>,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
        cancel: Option<&CancelToken>,
    ) -> Result<()> {
        if content.is_empty() {
            return Err(HarnessError::Other("empty user content".into()));
        }
        self.send_user_cancellable(Message::user_blocks(content), on_event, cancel)
            .await
    }

    async fn send_user_cancellable(
        &mut self,
        user_message: Message,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
        cancel: Option<&CancelToken>,
    ) -> Result<()> {
        let mut staged = self.messages.clone();
        staged.push(user_message);
        // Track which tool_use ids were sensitive so tool_result redaction can
        // strip them from durable history while live memory keeps the body.
        let mut turn_sensitive: Vec<String> = Vec::new();
        // Overwritten each provider round; only the final end_turn value is kept.
        #[allow(unused_assignments)]
        let mut last_usage: Option<Usage> = None;

        loop {
            Self::check_cancel(cancel)?;

            let request = TurnRequest {
                model: self.model.clone(),
                system: self.system.clone(),
                messages: staged.clone(),
                // A model catalogue is a capability contract, not just picker
                // decoration. Do not send function definitions to a text-only
                // model, and do not send an effort request to a model whose
                // provider has no wire-level effort control.
                tools: self.tools_for_model(),
                allow_tool_use: true,
                max_tokens: self.max_tokens,
                effort: self.effort_for_model(),
                thinking: true,
                provider_session: self.provider_session.clone(),
                interaction: self.provider_interaction.clone(),
                cancel: cancel.cloned(),
            };

            let completion = match self.provider.stream_turn(&request, &mut *on_event).await {
                Ok(c) => c,
                Err(e) => {
                    // Do not commit staged history — keep prior wire messages intact.
                    self.provider_session = None;
                    return Err(e);
                }
            };

            self.provider_session = completion.provider_session.clone();

            // Say so when the endpoint served something other than what was
            // asked for. Only on disagreement: a notice on every turn would be
            // noise, and this is the one fact no other surface can supply.
            if let Some(served) = completion.served_model.as_deref() {
                if !models_agree(&request.model, served) {
                    on_event(StreamEvent::ModelSubstituted {
                        requested: request.model.clone(),
                        served: served.to_string(),
                    });
                }
            }

            // Bill completed paid responses before a late cancel can discard the
            // staged wire history. Accounting must never abort a paid-for turn.
            if let Some(ledger) = &self.ledger {
                if let Ok(mut ledger) = ledger.lock() {
                    ledger.record(
                        self.provider.id(),
                        billed_model(&request.model, completion.served_model.as_deref()),
                        &completion,
                    );
                }
            }
            last_usage = Some(completion.usage.clone());

            Self::check_cancel(cancel)?;

            // Echo the assistant turn back verbatim — thinking signatures and
            // tool_use blocks both have to survive intact.
            staged.push(Message::assistant(completion.content.clone()));

            match completion.stop_reason.as_deref() {
                Some("end_turn") | None => {
                    self.sensitive_tool_ids.extend(turn_sensitive);
                    // Live memory keeps real tool bodies; persist path redacts.
                    self.messages = staged;
                    self.last_usage = last_usage;
                    return Ok(());
                }

                Some("tool_use") => {
                    let calls = tool_uses(&completion.content);
                    if calls.is_empty() {
                        return Err(HarnessError::Other(
                            "stop_reason was tool_use but no tool_use block was present".into(),
                        ));
                    }

                    Self::check_cancel(cancel)?;
                    // Only when a tool in *this* round will read it. Building it
                    // clones the entire conversation and walks it to redact
                    // sensitive tool bodies; doing that on every tool round
                    // because delegation happens to be configured spends a
                    // transcript-sized copy per file read, in sessions that may
                    // never delegate at all. It still runs immediately before
                    // the call that consumes it, so what the worker sees is
                    // exactly as fresh as it was.
                    let called: Vec<&str> = calls.iter().map(|call| call.name.as_str()).collect();
                    if self.tools.round_uses_context(&called) {
                        let mut sensitive_ids = self.sensitive_tool_ids.clone();
                        sensitive_ids.extend(turn_sensitive.iter().cloned());
                        let handoff_messages =
                            redact_sensitive_staged(staged.clone(), &sensitive_ids);
                        self.tools.update_context(&handoff_messages);
                    }
                    let outcomes = self.execute_tool_calls(&calls, on_event, cancel).await;
                    Self::check_cancel(cancel)?;

                    // Emission and wire order both follow the order the model
                    // asked in, never completion order.
                    let mut results = Vec::with_capacity(calls.len());
                    for (call, outcome) in calls.iter().zip(outcomes) {
                        if outcome.risk == ToolRisk::Sensitive {
                            turn_sensitive.push(call.id.clone());
                        }
                        if let Some(crate::tools::ToolMetadata::Delegation {
                            provider_id,
                            usage,
                            ..
                        }) = outcome.metadata.as_ref()
                        {
                            if let Some(ledger) = &self.ledger {
                                if let Ok(mut ledger) = ledger.lock() {
                                    ledger.record_external(provider_id, usage.as_ref());
                                }
                            }
                        }
                        let summary = if outcome.risk == ToolRisk::Sensitive {
                            "sensitive content (hidden)".to_string()
                        } else if let Some(label) =
                            outcome.metadata.as_ref().and_then(|m| m.delegation_label())
                        {
                            // Prefer the short provenance label; full body stays on wire.
                            label
                        } else {
                            summarize_tool_body(&outcome.body)
                        };
                        let delegation_diff = outcome
                            .metadata
                            .as_ref()
                            .and_then(|metadata| metadata.delegation_diff())
                            .map(str::to_string);
                        on_event(StreamEvent::ToolCallResult {
                            name: &call.name,
                            id: &call.id,
                            summary: &summary,
                            is_error: outcome.is_error,
                            path: outcome.path.as_deref(),
                            diff: outcome.diff.as_deref().or(delegation_diff.as_deref()),
                            metadata: outcome.metadata,
                        });
                        // Live staged history keeps the real body for the model.
                        results.push(tool_result(&call.id, &outcome.body, outcome.is_error));
                    }

                    // One user message carrying every result.
                    staged.push(Message::user_blocks(results));
                }

                // A server-side tool hit its iteration cap. Resend as-is; the
                // server picks up where it left off.
                Some("pause_turn") => continue,

                Some("max_tokens") => {
                    return Err(HarnessError::StoppedEarly(
                        "hit max_tokens — raise Agent::max_tokens or lower effort".into(),
                    ))
                }

                Some("refusal") => {
                    return Err(HarnessError::StoppedEarly(
                        "the model declined this request".into(),
                    ))
                }

                Some(other) => {
                    return Err(HarnessError::StoppedEarly(format!(
                        "unrecognized stop_reason: {other}"
                    )))
                }
            }
        }
    }

    /// Return only the tools the selected model advertises support for.
    ///
    /// Provider implementations still own the final wire conversion. This
    /// small gate keeps the agent loop from asking a text-only model to reason
    /// about a function schema it cannot use.
    fn tools_for_model(&self) -> Vec<crate::anthropic::types::ToolDef> {
        let supports_tools = self
            .provider
            .models()
            .into_iter()
            .find(|spec| spec.id == self.model)
            .map(|spec| spec.supports_tools)
            .unwrap_or(true);
        if supports_tools {
            self.tools.definitions()
        } else {
            Vec::new()
        }
    }

    /// Return an effort request only when the selected model exposes one.
    ///
    /// `Agent::effort` remains a stable session value for persistence and the
    /// UI, but providers without an effort control must receive `None` rather
    /// than a plausible-looking `high` field that a vendor may reject or
    /// interpret differently.
    fn effort_for_model(&self) -> Option<String> {
        let supports_effort = self
            .provider
            .models()
            .into_iter()
            .find(|spec| spec.id == self.model)
            .map(|spec| !spec.efforts.is_empty())
            .unwrap_or(true);
        supports_effort.then(|| self.effort.clone())
    }

    fn check_cancel(cancel: Option<&CancelToken>) -> Result<()> {
        if cancel.map(|c| c.is_cancelled()).unwrap_or(false) {
            Err(HarnessError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Run every tool the model asked for, returning one outcome per call **in
    /// call order** regardless of the order they finish in.
    ///
    /// Ungated calls run concurrently. They are independent by construction —
    /// the model issued them all before seeing any result — so serializing them
    /// only ever costs wall-clock, and a one-second `web_search` should not
    /// stall three instant file reads.
    ///
    /// Gated calls stay strictly sequential and run after the concurrent batch.
    /// Two reasons: the user must see one approval card at a time, and two
    /// writes to the same path must never race. Running them last also means a
    /// read in the same batch observes the pre-write file deterministically,
    /// rather than depending on who won.
    async fn execute_tool_calls(
        &self,
        calls: &[crate::anthropic::types::ToolUse],
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
        cancel: Option<&CancelToken>,
    ) -> Vec<ToolCallOutcome> {
        let mut slots: Vec<Option<ToolCallOutcome>> = (0..calls.len()).map(|_| None).collect();

        if cancel.map(|c| c.is_cancelled()).unwrap_or(false) {
            return slots
                .into_iter()
                .map(|_| ToolCallOutcome::failed("turn cancelled before tool ran", ToolRisk::Read))
                .collect();
        }

        // Prepare once, up front, for every call: preview, path, and pre-image
        // fingerprint must be the same plan that later executes. Preparing the
        // whole batch is also what tells us which calls need a human.
        let mut auto: Vec<(usize, PreparedToolCall)> = Vec::new();
        let mut gated: Vec<(usize, PreparedToolCall)> = Vec::new();
        let mut question: Option<(usize, PreparedToolCall)> = None;
        for (index, call) in calls.iter().enumerate() {
            let prepared = match self.tools.prepare(&call.name, call.input.clone()) {
                Ok(prepared) => prepared,
                Err(message) => {
                    slots[index] = Some(ToolCallOutcome::failed(
                        format!("cannot prepare `{}`: {message}", call.name),
                        ToolRisk::Read,
                    ));
                    continue;
                }
            };

            if let Some(metadata) = prepared.metadata.clone() {
                on_event(StreamEvent::ToolCallUpdate {
                    name: &call.name,
                    id: &call.id,
                    metadata,
                });
            }

            if call.name == ASK_USER_TOOL {
                if question.is_some() {
                    slots[index] = Some(ToolCallOutcome::failed(
                        "ask_user accepts one question at a time",
                        ToolRisk::Read,
                    ));
                } else {
                    question = Some((index, prepared));
                }
            } else if prepared.risk.requires_approval() {
                gated.push((index, prepared));
            } else {
                auto.push((index, prepared));
            }
        }

        // A question controls what the next tool batch should do. Resolve it
        // before ordinary reads or approvals rather than letting a concurrent
        // call run against a decision the user has not made yet.
        if let Some((index, prepared)) = question {
            slots[index] = Some(
                self.run_question_call(&calls[index], prepared, on_event, cancel)
                    .await,
            );
        }

        if !auto.is_empty() {
            let planned: Vec<(usize, ToolRisk, Option<String>, Option<String>)> = auto
                .iter()
                .map(|(i, p)| {
                    (
                        *i,
                        p.risk,
                        (!p.preview.path.is_empty()).then(|| p.preview.path.clone()),
                        (!p.preview.diff.is_empty()).then(|| p.preview.diff.clone()),
                    )
                })
                .collect();
            let running = auto
                .into_iter()
                .map(|(_, prepared)| self.tools.execute_prepared(prepared));

            let finished = tokio::select! {
                biased;
                _ = wait_cancel(cancel) => None,
                results = futures_util::future::join_all(running) => Some(results),
            };

            match finished {
                Some(results) => {
                    for ((index, risk, path, diff), exec) in planned.into_iter().zip(results) {
                        slots[index] = Some(match exec {
                            Ok(outcome) => ToolCallOutcome {
                                body: outcome.body,
                                is_error: false,
                                risk,
                                path,
                                diff,
                                metadata: outcome.metadata,
                            },
                            Err(message) => {
                                let mut failed = ToolCallOutcome::failed(message, risk);
                                failed.path = path;
                                failed.diff = diff;
                                failed
                            }
                        });
                    }
                }
                None => {
                    for (index, risk, _, _) in planned {
                        slots[index] = Some(ToolCallOutcome::failed(
                            "turn cancelled before tool finished",
                            risk,
                        ));
                    }
                }
            }
        }

        for (index, prepared) in gated {
            slots[index] = Some(
                self.run_gated_call(&calls[index], prepared, on_event, cancel)
                    .await,
            );
        }

        slots
            .into_iter()
            .enumerate()
            .map(|(index, slot)| {
                slot.unwrap_or_else(|| {
                    ToolCallOutcome::failed(
                        format!(
                            "internal error: no outcome recorded for `{}`",
                            calls[index].name
                        ),
                        ToolRisk::Read,
                    )
                })
            })
            .collect()
    }

    /// One interactive question: reserve the waiter, notify the front-end,
    /// then turn the answer into the ordinary model-visible tool result.
    async fn run_question_call(
        &self,
        call: &crate::anthropic::types::ToolUse,
        prepared: PreparedToolCall,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
        cancel: Option<&CancelToken>,
    ) -> ToolCallOutcome {
        if cancel.map(|token| token.is_cancelled()).unwrap_or(false) {
            return ToolCallOutcome::failed(
                "turn cancelled before waiting for an answer",
                ToolRisk::Read,
            );
        }

        let input = match prepared.plain_input() {
            Some(input) => input,
            None => {
                return ToolCallOutcome::failed(
                    "internal error: ask_user prepared kind mismatch",
                    ToolRisk::Read,
                )
            }
        };
        let question_id = new_id("question");
        let request = match parse_question_input(input, &question_id, &call.id) {
            Ok(request) => request,
            Err(message) => return ToolCallOutcome::failed(message, ToolRisk::Read),
        };

        self.questioner.prepare(&question_id).await;
        on_event(StreamEvent::QuestionNeeded {
            question_id: request.question_id.clone(),
            tool_call_id: request.tool_call_id.clone(),
            prompt: request.prompt.clone(),
            choices: request.choices.clone(),
            multiple: request.multiple,
            placeholder: request.placeholder.clone(),
        });

        let answer = tokio::select! {
            biased;
            _ = wait_cancel(cancel) => Err("turn cancelled while waiting for an answer".into()),
            result = self.questioner.answer(&request) => result,
        };

        match answer {
            Ok(answer) if !answer.trim().is_empty() => ToolCallOutcome {
                body: answer,
                is_error: false,
                risk: ToolRisk::Read,
                path: None,
                diff: None,
                metadata: None,
            },
            Ok(_) => ToolCallOutcome::failed("the user submitted an empty answer", ToolRisk::Read),
            Err(message) => ToolCallOutcome::failed(message, ToolRisk::Read),
        }
    }

    /// One approval-gated call: prompt, wait, then execute if allowed.
    async fn run_gated_call(
        &self,
        call: &crate::anthropic::types::ToolUse,
        prepared: PreparedToolCall,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
        cancel: Option<&CancelToken>,
    ) -> ToolCallOutcome {
        if cancel.map(|c| c.is_cancelled()).unwrap_or(false) {
            return ToolCallOutcome::failed("turn cancelled before tool ran", prepared.risk);
        }

        let risk = prepared.risk;
        // The target is what the user was actually shown — a file path, or the
        // command line itself — so a session grant covers exactly that.
        let target = prepared.preview.path.clone();

        let outcome = match self.policy.lock() {
            Ok(policy) => policy.decide(&call.name, &target, risk, prepared.auto_eligible),
            // A poisoned lock must not become an open door.
            Err(_) => PolicyOutcome::Ask,
        };

        match outcome {
            PolicyOutcome::Allow => return self.run_prepared(prepared, risk, cancel).await,
            PolicyOutcome::Block(reason) => return ToolCallOutcome::failed(reason, risk),
            PolicyOutcome::Ask => {}
        }

        {
            let mut preview = prepared.preview.clone();
            // Hide sensitive diffs/summaries from durable UI cards.
            if risk == ToolRisk::Sensitive {
                preview.diff.clear();
                if preview.summary.is_empty() {
                    preview.summary = format!("Access sensitive path {}", preview.path);
                }
            }
            let approval_id = new_id("approval");
            // Register the waiter before the UI sees the event.
            self.approver.prepare(&approval_id).await;

            on_event(StreamEvent::ApprovalNeeded {
                approval_id: approval_id.clone(),
                tool_name: call.name.clone(),
                tool_call_id: call.id.clone(),
                risk,
                path: preview.path.clone(),
                summary: preview.summary.clone(),
                diff: preview.diff.clone(),
            });

            let summary_for_deny = preview.summary.clone();
            let request = ApprovalRequest {
                approval_id,
                tool_name: call.name.clone(),
                tool_call_id: call.id.clone(),
                risk,
                preview,
            };

            let decision = tokio::select! {
                biased;
                _ = wait_cancel(cancel) => ApprovalDecision::Deny,
                d = self.approver.decide(&request) => d,
            };

            match decision {
                ApprovalDecision::AllowOnce => {}
                ApprovalDecision::AllowSession => {
                    // Record against the exact target that was on the card.
                    if let Ok(mut policy) = self.policy.lock() {
                        policy.trust(&call.name, &target);
                    }
                }
                ApprovalDecision::Deny => {
                    if cancel.map(|c| c.is_cancelled()).unwrap_or(false) {
                        return ToolCallOutcome::failed("turn cancelled during approval", risk);
                    }
                    return ToolCallOutcome::failed(
                        format!(
                            "user denied permission to run `{}` ({summary_for_deny})",
                            call.name
                        ),
                        risk,
                    );
                }
            }

            if cancel.map(|c| c.is_cancelled()).unwrap_or(false) {
                return ToolCallOutcome::failed("turn cancelled during approval", risk);
            }
        }

        self.run_prepared(prepared, risk, cancel).await
    }

    /// Execute an approved call, racing the cancel token.
    async fn run_prepared(
        &self,
        prepared: PreparedToolCall,
        risk: ToolRisk,
        cancel: Option<&CancelToken>,
    ) -> ToolCallOutcome {
        let path = (!prepared.preview.path.is_empty()).then(|| prepared.preview.path.clone());
        let diff = (!prepared.preview.diff.is_empty()).then(|| prepared.preview.diff.clone());
        let exec = tokio::select! {
            biased;
            _ = wait_cancel(cancel) => {
                return ToolCallOutcome::failed("turn cancelled before tool finished", risk);
            }
            result = self.tools.execute_prepared(prepared) => result,
        };

        match exec {
            Ok(outcome) => ToolCallOutcome {
                body: outcome.body,
                is_error: false,
                risk,
                path,
                diff,
                metadata: outcome.metadata,
            },
            Err(message) => {
                let mut failed = ToolCallOutcome::failed(message, risk);
                failed.path = path;
                failed.diff = diff;
                failed
            }
        }
    }
}

/// One tool call's result, kept beside the risk that produced it so the caller
/// can decide about redaction and UI summaries without re-querying the registry.
struct ToolCallOutcome {
    body: String,
    is_error: bool,
    risk: ToolRisk,
    path: Option<String>,
    diff: Option<String>,
    metadata: Option<crate::tools::ToolMetadata>,
}

impl ToolCallOutcome {
    fn failed(body: impl Into<String>, risk: ToolRisk) -> Self {
        Self {
            body: body.into(),
            is_error: true,
            risk,
            path: None,
            diff: None,
            metadata: None,
        }
    }
}

/// Short one-line preview for UI / CLI tool result markers.
/// Which model a turn is billed to.
///
/// The requested name, unless the endpoint served something genuinely different.
///
/// Not simply `served_model`: providers routinely answer an alias with its dated
/// build, and billing those apart would split one model across two ledger rows
/// while leaving both unmatched by a price book keyed on the alias. `models_agree`
/// already draws that line for the substitution warning, so accounting draws it
/// the same way — one rule, one place. A real substitution does bill to what ran,
/// because that is what spent the money.
fn billed_model<'a>(requested: &'a str, served: Option<&'a str>) -> &'a str {
    match served {
        Some(served) if !models_agree(requested, served) => served,
        _ => requested,
    }
}

/// Whether a served model name is the one that was requested.
///
/// Deliberately loose. Providers routinely answer with a more specific name than
/// the alias you asked for — `claude-opus-5` is served by `claude-opus-5-<date>`,
/// and gateways append their own suffixes. Treating that as a substitution would
/// fire a warning on every healthy turn, which would train people to ignore the
/// one that matters. So one name containing the other counts as agreement, and
/// only a genuinely different family is reported.
fn models_agree(requested: &str, served: &str) -> bool {
    let requested = requested.trim().to_ascii_lowercase();
    let served = served.trim().to_ascii_lowercase();
    if requested.is_empty() || served.is_empty() {
        return true;
    }
    requested == served || served.starts_with(&requested) || requested.starts_with(&served)
}

fn summarize_tool_body(body: &str) -> String {
    const MAX: usize = 160;
    let flat: String = body
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if flat.chars().count() <= MAX {
        return flat;
    }
    let truncated: String = flat.chars().take(MAX.saturating_sub(1)).collect();
    format!("{truncated}…")
}

fn redact_sensitive_staged(messages: Vec<Message>, sensitive_ids: &[String]) -> Vec<Message> {
    if sensitive_ids.is_empty() {
        return messages;
    }
    let mut out = messages;
    for msg in &mut out {
        if msg.role != "user" {
            continue;
        }
        for block in &mut msg.content {
            let is_result = block.get("type").and_then(|t| t.as_str()) == Some("tool_result");
            if !is_result {
                continue;
            }
            let id = block
                .get("tool_use_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if sensitive_ids.iter().any(|s| s == id) {
                block["content"] = serde_json::Value::String(REDACTED_SENSITIVE_RESULT.into());
            }
        }
    }
    out
}

#[cfg(test)]
mod billing_tests {
    use super::billed_model;

    #[test]
    fn a_dated_build_bills_to_the_alias_that_was_asked_for() {
        // Otherwise one model becomes two ledger rows, and neither matches a
        // price book keyed on the alias.
        assert_eq!(
            billed_model("claude-opus-5", Some("claude-opus-5-20260101")),
            "claude-opus-5"
        );
        assert_eq!(
            billed_model("gpt-5.6-sol", Some("gpt-5.6-sol")),
            "gpt-5.6-sol"
        );
    }

    #[test]
    fn a_real_substitution_bills_to_what_actually_ran() {
        assert_eq!(
            billed_model("claude-opus-5", Some("claude-haiku-4-5")),
            "claude-haiku-4-5"
        );
    }

    #[test]
    fn a_silent_endpoint_bills_to_the_request() {
        assert_eq!(billed_model("gpt-5.6-sol", None), "gpt-5.6-sol");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::types::Usage;
    use crate::auth::AuthStatus;
    use crate::provider::Completion;
    use crate::provider::ModelSpec;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    struct FakeProvider {
        calls: AtomicUsize,
        fail_after: Option<usize>,
        stop: &'static str,
    }

    #[async_trait]
    impl Provider for FakeProvider {
        fn id(&self) -> &str {
            "fake"
        }

        fn default_model(&self) -> &str {
            "fake-model"
        }

        fn auth_status(&self) -> AuthStatus {
            AuthStatus::Ready { account: None }
        }

        async fn stream_turn(
            &self,
            _req: &TurnRequest,
            on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
        ) -> Result<Completion> {
            let n = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            if self.fail_after == Some(n) {
                return Err(HarnessError::Other("provider boom".into()));
            }
            on_event(StreamEvent::Text("hi"));
            Ok(Completion {
                content: vec![json!({ "type": "text", "text": "hi" })],
                stop_reason: Some(self.stop.into()),
                usage: Usage::default(),
                usage_available: true,
                limits: None,
                served_model: None,
                provider_session: None,
            })
        }
    }

    type CapabilityObservations = Arc<Mutex<Vec<(Option<String>, usize)>>>;

    struct CapabilityProvider {
        seen: CapabilityObservations,
    }

    #[async_trait]
    impl Provider for CapabilityProvider {
        fn id(&self) -> &str {
            "capability-test"
        }

        fn default_model(&self) -> &str {
            "text-only-model"
        }

        fn models(&self) -> Vec<ModelSpec> {
            vec![ModelSpec {
                id: "text-only-model".into(),
                efforts: Vec::new(),
                context_window: 16_000,
                supports_tools: false,
                supports_vision: false,
            }]
        }

        fn auth_status(&self) -> AuthStatus {
            AuthStatus::Ready { account: None }
        }

        async fn stream_turn(
            &self,
            req: &TurnRequest,
            _on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
        ) -> Result<Completion> {
            self.seen
                .lock()
                .unwrap()
                .push((req.effort.clone(), req.tools.len()));
            Ok(Completion {
                content: vec![json!({ "type": "text", "text": "done" })],
                stop_reason: Some("end_turn".into()),
                usage: Usage::default(),
                usage_available: true,
                limits: None,
                served_model: None,
                provider_session: None,
            })
        }
    }

    struct NoopTool;

    #[async_trait]
    impl crate::tools::Tool for NoopTool {
        fn name(&self) -> &str {
            "noop"
        }

        fn description(&self) -> &str {
            "test tool"
        }

        fn input_schema(&self) -> serde_json::Value {
            json!({ "type": "object", "properties": {} })
        }

        async fn run(
            &self,
            _input: serde_json::Value,
        ) -> std::result::Result<crate::tools::ToolOutcome, String> {
            Ok(crate::tools::ToolOutcome::text("noop"))
        }
    }

    #[tokio::test]
    async fn model_capabilities_gate_effort_and_tool_definitions() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn Provider> = Arc::new(CapabilityProvider { seen: seen.clone() });
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(NoopTool));

        let mut agent = Agent::new(provider, tools);
        let mut sink = |_ev: StreamEvent<'_>| {};
        agent.send("hello", &mut sink).await.unwrap();

        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &[(None, 0)],
            "unsupported capabilities must not reach the provider"
        );
    }

    #[tokio::test]
    async fn successful_turn_commits_wire_history() {
        let provider: Arc<dyn Provider> = Arc::new(FakeProvider {
            calls: AtomicUsize::new(0),
            fail_after: None,
            stop: "end_turn",
        });
        let mut agent = Agent::new(provider, ToolRegistry::new());
        let mut sink = |_ev: StreamEvent<'_>| {};
        agent.send("hello", &mut sink).await.unwrap();
        assert_eq!(agent.messages.len(), 2);
    }

    #[tokio::test]
    async fn compaction_replaces_history_and_clears_stale_usage() {
        let provider: Arc<dyn Provider> = Arc::new(FakeProvider {
            calls: AtomicUsize::new(0),
            fail_after: None,
            stop: "end_turn",
        });
        let mut agent = Agent::new(provider, ToolRegistry::new());
        agent.messages = (0..4)
            .map(|index| Message::user_text(format!("old message {index}")))
            .collect();
        agent.last_usage = Some(Usage {
            input_tokens: 12_345,
            ..Usage::default()
        });

        let summary = agent.compact_context().await.unwrap();

        assert_eq!(summary, "hi");
        assert_eq!(agent.messages.len(), 2);
        assert!(agent.last_usage.is_none());
    }

    #[tokio::test]
    async fn provider_error_does_not_commit_staged_history() {
        let provider: Arc<dyn Provider> = Arc::new(FakeProvider {
            calls: AtomicUsize::new(0),
            fail_after: Some(0),
            stop: "end_turn",
        });
        let mut agent = Agent::new(provider, ToolRegistry::new());
        agent.messages.push(Message::user_text("prior"));
        let prior_len = agent.messages.len();
        let mut sink = |_ev: StreamEvent<'_>| {};
        let err = agent.send("new", &mut sink).await.unwrap_err();
        assert!(matches!(err, HarnessError::Other(_)));
        assert_eq!(agent.messages.len(), prior_len);
        assert_eq!(agent.messages[0].role, "user");
    }

    /// A tool that sleeps for `delay_ms` and reports its own name, so a batch
    /// can be arranged to finish in the reverse of the order it was called in.
    struct SlowTool {
        name: &'static str,
        delay_ms: u64,
        /// Records completion order across the whole batch.
        finished: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl crate::tools::Tool for SlowTool {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "test tool"
        }
        fn input_schema(&self) -> serde_json::Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn run(
            &self,
            _input: serde_json::Value,
        ) -> std::result::Result<crate::tools::ToolOutcome, String> {
            tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            if let Ok(mut f) = self.finished.lock() {
                f.push(self.name);
            }
            Ok(crate::tools::ToolOutcome::text(self.name.to_string()))
        }
    }

    /// Emits a `tool_use` turn on the first call and `end_turn` afterwards, so
    /// the agent runs exactly one batch of tools.
    struct ToolCallingProvider {
        calls: AtomicUsize,
        tools: Vec<&'static str>,
    }

    #[async_trait]
    impl Provider for ToolCallingProvider {
        fn id(&self) -> &str {
            "fake"
        }
        fn default_model(&self) -> &str {
            "fake-model"
        }
        fn auth_status(&self) -> AuthStatus {
            AuthStatus::Ready { account: None }
        }
        async fn stream_turn(
            &self,
            _req: &TurnRequest,
            _on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
        ) -> Result<Completion> {
            let n = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            if n > 0 {
                return Ok(Completion {
                    content: vec![json!({ "type": "text", "text": "done" })],
                    stop_reason: Some("end_turn".into()),
                    usage: Usage::default(),
                    usage_available: true,
                    limits: None,
                    served_model: None,
                    provider_session: None,
                });
            }
            let content = self
                .tools
                .iter()
                .map(|name| {
                    json!({ "type": "tool_use", "id": format!("call_{name}"), "name": name, "input": {} })
                })
                .collect();
            Ok(Completion {
                content,
                stop_reason: Some("tool_use".into()),
                usage: Usage::default(),
                usage_available: true,
                limits: None,
                served_model: None,
                provider_session: None,
            })
        }
    }

    struct AskingProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Provider for AskingProvider {
        fn id(&self) -> &str {
            "fake"
        }

        fn default_model(&self) -> &str {
            "fake-model"
        }

        fn auth_status(&self) -> AuthStatus {
            AuthStatus::Ready { account: None }
        }

        async fn stream_turn(
            &self,
            _req: &TurnRequest,
            _on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
        ) -> Result<Completion> {
            let call = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            if call == 0 {
                Ok(Completion {
                    content: vec![json!({
                        "type": "tool_use",
                        "id": "ask-1",
                        "name": "ask_user",
                        "input": {
                            "question": "Which layout should I use?",
                            "choices": ["Compact", "Spacious"]
                        }
                    })],
                    stop_reason: Some("tool_use".into()),
                    usage: Usage::default(),
                    usage_available: true,
                    limits: None,
                    served_model: None,
                    provider_session: None,
                })
            } else {
                Ok(Completion {
                    content: vec![json!({ "type": "text", "text": "continued" })],
                    stop_reason: Some("end_turn".into()),
                    usage: Usage::default(),
                    usage_available: true,
                    limits: None,
                    served_model: None,
                    provider_session: None,
                })
            }
        }
    }

    struct RecordingQuestioner {
        seen: Arc<Mutex<Vec<crate::tools::QuestionRequest>>>,
    }

    #[async_trait]
    impl Questioner for RecordingQuestioner {
        async fn answer(
            &self,
            request: &crate::tools::QuestionRequest,
        ) -> std::result::Result<String, String> {
            self.seen.lock().unwrap().push(request.clone());
            Ok("Compact".into())
        }
    }

    #[tokio::test]
    async fn ask_user_emits_a_question_and_resumes_the_same_turn() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn Provider> = Arc::new(AskingProvider {
            calls: AtomicUsize::new(0),
        });
        let mut tools = ToolRegistry::new();
        crate::tools::register_question_tool(&mut tools);
        let mut agent = Agent::new(provider, tools)
            .with_questioner(Arc::new(RecordingQuestioner { seen: seen.clone() }));
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let events_for_sink = events.clone();
        let mut sink = move |event: StreamEvent<'_>| {
            if let Ok(mut events) = events_for_sink.lock() {
                match event {
                    StreamEvent::QuestionNeeded { prompt, .. } => {
                        events.push(format!("question:{prompt}"));
                    }
                    StreamEvent::ToolCallResult { summary, .. } => {
                        events.push(format!("result:{summary}"));
                    }
                    _ => {}
                }
            }
        };

        agent.send("choose a layout", &mut sink).await.unwrap();

        let questions = seen.lock().unwrap();
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].prompt, "Which layout should I use?");
        assert_eq!(questions[0].choices, ["Compact", "Spacious"]);
        drop(questions);
        assert_eq!(
            events.lock().unwrap().as_slice(),
            ["question:Which layout should I use?", "result:Compact",]
        );
        assert_eq!(agent.messages.len(), 4);
        assert_eq!(agent.messages[2].role, "user");
        assert_eq!(agent.messages[2].content[0]["content"], "Compact");
    }

    struct ContextCaptureTool {
        seen: Arc<Mutex<Vec<Message>>>,
    }

    #[async_trait]
    impl crate::tools::Tool for ContextCaptureTool {
        fn name(&self) -> &str {
            "capture_context"
        }
        fn description(&self) -> &str {
            "test context projection"
        }
        fn input_schema(&self) -> serde_json::Value {
            json!({ "type": "object", "properties": {} })
        }
        fn update_context(&self, messages: &[Message]) {
            if let Ok(mut seen) = self.seen.lock() {
                *seen = messages.to_vec();
            }
        }
        fn uses_context(&self) -> bool {
            true
        }
        async fn run(
            &self,
            _input: serde_json::Value,
        ) -> std::result::Result<crate::tools::ToolOutcome, String> {
            Ok(crate::tools::ToolOutcome::text("captured"))
        }
    }

    #[tokio::test]
    async fn a_round_without_a_contextual_tool_does_not_build_the_context() {
        // Building it clones and redacts the whole conversation. Doing that
        // because a contextual tool is *registered*, rather than because one is
        // about to run, spends a transcript-sized copy on every file read in
        // sessions that never delegate.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(ContextCaptureTool { seen: seen.clone() }));
        tools.register(Arc::new(NoopTool));

        // The model calls the plain tool; the contextual one is registered but
        // not invoked.
        let provider: Arc<dyn Provider> = Arc::new(ToolCallingProvider {
            calls: AtomicUsize::new(0),
            tools: vec!["noop"],
        });
        let mut agent = Agent::new(provider, tools);

        let mut sink = |_ev: StreamEvent<'_>| {};
        agent
            .send("do something ordinary", &mut sink)
            .await
            .unwrap();

        assert!(
            seen.lock().unwrap().is_empty(),
            "no contextual tool ran, so no context should have been built"
        );
    }

    #[tokio::test]
    async fn contextual_tools_receive_current_staged_history_with_secrets_redacted() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(ContextCaptureTool { seen: seen.clone() }));
        let provider: Arc<dyn Provider> = Arc::new(ToolCallingProvider {
            calls: AtomicUsize::new(0),
            tools: vec!["capture_context"],
        });
        let mut agent = Agent::new(provider, tools);
        agent.messages = vec![
            Message::assistant(vec![json!({
                "type": "tool_use",
                "id": "sensitive-1",
                "name": "read_file",
                "input": { "path": ".env" }
            })]),
            Message::user_blocks(vec![tool_result(
                "sensitive-1",
                "API_KEY=top-secret",
                false,
            )]),
        ];
        agent.sensitive_tool_ids.push("sensitive-1".into());

        let mut sink = |_ev: StreamEvent<'_>| {};
        agent.send("current user prompt", &mut sink).await.unwrap();

        let json = serde_json::to_string(&*seen.lock().unwrap()).unwrap();
        assert!(json.contains("current user prompt"));
        assert!(json.contains("capture_context"));
        assert!(json.contains(REDACTED_SENSITIVE_RESULT));
        assert!(!json.contains("top-secret"));
    }

    /// The invariant that makes concurrency safe: tools may finish in any
    /// order, but `tool_result` blocks must come back in the order the model
    /// asked for them.
    #[tokio::test]
    async fn parallel_tool_results_keep_call_order() {
        let finished = Arc::new(Mutex::new(Vec::new()));
        let mut tools = ToolRegistry::new();
        // Called slow → medium → fast; they finish in exactly the reverse.
        for (name, delay) in [("slow", 60u64), ("medium", 30), ("fast", 1)] {
            tools.register(Arc::new(SlowTool {
                name,
                delay_ms: delay,
                finished: finished.clone(),
            }));
        }

        let provider: Arc<dyn Provider> = Arc::new(ToolCallingProvider {
            calls: AtomicUsize::new(0),
            tools: vec!["slow", "medium", "fast"],
        });
        let mut agent = Agent::new(provider, tools);
        let mut sink = |_ev: StreamEvent<'_>| {};
        agent.send("go", &mut sink).await.unwrap();

        assert_eq!(
            finished.lock().unwrap().clone(),
            vec!["fast", "medium", "slow"],
            "tools must actually have run concurrently and finished out of order"
        );

        // user, assistant(tool_use), user(tool_result x3), assistant(end_turn)
        let results = &agent.messages[2];
        assert_eq!(results.role, "user");
        let ids: Vec<&str> = results
            .content
            .iter()
            .map(|b| b["tool_use_id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["call_slow", "call_medium", "call_fast"]);
        let bodies: Vec<&str> = results
            .content
            .iter()
            .map(|b| b["content"].as_str().unwrap())
            .collect();
        assert_eq!(bodies, vec!["slow", "medium", "fast"]);
    }

    /// Wall-clock proof, not just ordering: three 80 ms tools overlap.
    #[tokio::test]
    async fn independent_tools_overlap_instead_of_serializing() {
        let finished = Arc::new(Mutex::new(Vec::new()));
        let mut tools = ToolRegistry::new();
        for name in ["a", "b", "c"] {
            tools.register(Arc::new(SlowTool {
                name,
                delay_ms: 80,
                finished: finished.clone(),
            }));
        }
        let provider: Arc<dyn Provider> = Arc::new(ToolCallingProvider {
            calls: AtomicUsize::new(0),
            tools: vec!["a", "b", "c"],
        });
        let mut agent = Agent::new(provider, tools);
        let mut sink = |_ev: StreamEvent<'_>| {};

        let started = std::time::Instant::now();
        agent.send("go", &mut sink).await.unwrap();
        let elapsed = started.elapsed();

        assert_eq!(finished.lock().unwrap().len(), 3);
        // Serial would be ~240 ms. Generous bound so a loaded CI box does not
        // flake, but still far below the serial floor.
        assert!(
            elapsed < std::time::Duration::from_millis(200),
            "tools serialized: {elapsed:?}"
        );
    }

    /// Gated tools must not join the concurrent batch — one approval card at a
    /// time, and no two writes racing for the same path.
    #[tokio::test]
    async fn gated_tools_run_sequentially_after_the_concurrent_batch() {
        use crate::tools::approval::AllowApprover;
        use crate::tools::prepared::PreparedToolCall;

        struct GatedTool {
            name: &'static str,
            order: Arc<Mutex<Vec<String>>>,
        }

        #[async_trait]
        impl crate::tools::Tool for GatedTool {
            fn name(&self) -> &str {
                self.name
            }
            fn description(&self) -> &str {
                "gated test tool"
            }
            fn input_schema(&self) -> serde_json::Value {
                json!({ "type": "object", "properties": {} })
            }
            fn risk(&self) -> ToolRisk {
                ToolRisk::Write
            }
            fn prepare(
                &self,
                input: serde_json::Value,
            ) -> std::result::Result<PreparedToolCall, String> {
                Ok(PreparedToolCall::plain(self.name, ToolRisk::Write, input))
            }
            async fn run(
                &self,
                _input: serde_json::Value,
            ) -> std::result::Result<crate::tools::ToolOutcome, String> {
                if let Ok(mut o) = self.order.lock() {
                    o.push(format!("enter:{}", self.name));
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                if let Ok(mut o) = self.order.lock() {
                    o.push(format!("exit:{}", self.name));
                }
                Ok(crate::tools::ToolOutcome::text(self.name.to_string()))
            }
        }

        let order = Arc::new(Mutex::new(Vec::new()));
        let mut tools = ToolRegistry::new();
        for name in ["w1", "w2"] {
            tools.register(Arc::new(GatedTool {
                name,
                order: order.clone(),
            }));
        }
        let provider: Arc<dyn Provider> = Arc::new(ToolCallingProvider {
            calls: AtomicUsize::new(0),
            tools: vec!["w1", "w2"],
        });
        let mut agent = Agent::new(provider, tools).with_approver(Arc::new(AllowApprover));
        let mut sink = |_ev: StreamEvent<'_>| {};
        agent.send("go", &mut sink).await.unwrap();

        assert_eq!(
            order.lock().unwrap().clone(),
            vec!["enter:w1", "exit:w1", "enter:w2", "exit:w2"],
            "gated writes must not overlap"
        );
    }

    #[tokio::test]
    async fn cancel_token_aborts_before_provider_call() {
        let provider: Arc<dyn Provider> = Arc::new(FakeProvider {
            calls: AtomicUsize::new(0),
            fail_after: None,
            stop: "end_turn",
        });
        let mut agent = Agent::new(provider, ToolRegistry::new());
        let cancel = CancelToken::new();
        cancel.cancel();
        let mut sink = |_ev: StreamEvent<'_>| {};
        let err = agent
            .send_cancellable("hello", &mut sink, Some(&cancel))
            .await
            .unwrap_err();
        assert!(matches!(err, HarnessError::Cancelled));
        assert!(agent.messages.is_empty());
    }

    /// A warning nobody can trust is worse than no warning: if a healthy turn
    /// trips it, people learn to ignore the one that matters.
    #[test]
    fn a_more_specific_served_name_is_not_a_substitution() {
        // Providers routinely answer an alias with a dated build.
        assert!(models_agree("claude-opus-5", "claude-opus-5-20260514"));
        assert!(models_agree("deepseek-v4-flash", "deepseek-v4-flash"));
        assert!(models_agree("gpt-5.6-sol", "gpt-5.6-sol-high"));
        // Order does not matter — some endpoints answer with the shorter name.
        assert!(models_agree("claude-opus-5-20260514", "claude-opus-5"));
        // Case and stray whitespace are not a model identity change either.
        assert!(models_agree("Claude-Opus-5", " claude-opus-5 "));
    }

    #[test]
    fn a_different_family_is_reported() {
        assert!(!models_agree("claude-opus-5", "deepseek-v4-flash"));
        assert!(!models_agree("deepseek-v4-pro", "deepseek-v4-flash"));
        assert!(!models_agree("gpt-5.6-sol", "claude-opus-5"));
    }

    #[test]
    fn silence_is_never_reported_as_a_substitution() {
        // An endpoint that names no model has not disagreed with anything.
        assert!(models_agree("claude-opus-5", ""));
        assert!(models_agree("", "claude-opus-5"));
    }
}
