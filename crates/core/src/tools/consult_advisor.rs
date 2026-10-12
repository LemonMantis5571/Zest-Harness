//! `consult_advisor`: the working model asks a stronger model for advice.
//!
//! The provider-independent parts (rendering, the request, resolution) live in
//! [`crate::advisor`]. This is the tool surface: the approval preview, the
//! per-request limit, the ledger records, and turning failures into tool errors
//! so the working model carries on without advice.

use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::advisor::{render_transcript, request_index, Advisor, ADVISOR_TOOL};
use crate::anthropic::types::Message;
use crate::provider::{Completion, StreamEvent, TurnRequest};
use crate::usage::{Ledger, TaskTraceGuard};

use super::approval::{ApprovalPreview, ToolRisk};
use super::prepared::PreparedToolCall;
use super::{Tool, ToolOutcome};

/// How much of the advice the transcript row keeps. The model gets all of it.
const SUMMARY_CHARS: usize = 4_000;

pub struct ConsultAdvisor {
    advisor: Advisor,
    /// The advisor is on a different provider than the chat, so a consultation
    /// sends the conversation to a second company and must be approved.
    cross_provider: bool,
    ledger: Option<Arc<Mutex<Ledger>>>,
    description: String,
    /// Redacted conversation, refreshed by the agent right before each call.
    context: RwLock<Vec<Message>>,
    /// `(request index, consultations during that request)`.
    uses: Mutex<(usize, u32)>,
}

impl ConsultAdvisor {
    pub fn new(
        advisor: Advisor,
        chat_provider_id: &str,
        ledger: Option<Arc<Mutex<Ledger>>>,
    ) -> Self {
        let description = format!(
            "Ask a stronger model ({}) for advice. It sees this whole conversation, so ask a short, \
             specific question instead of repeating context. Use it before committing to an approach \
             on a non-trivial task, when the same error keeps coming back, and before declaring a \
             multi-step task done. It cannot run tools or change files. At most {} consultation{} \
             per user request.",
            advisor.model,
            advisor.max_uses,
            if advisor.max_uses == 1 { "" } else { "s" }
        );
        Self {
            cross_provider: advisor.provider_id != chat_provider_id,
            advisor,
            ledger,
            description,
            context: RwLock::new(Vec::new()),
            uses: Mutex::new((0, 0)),
        }
    }

    /// `provider · model`: the row label, and what an approval grant covers.
    pub fn approval_target(&self) -> String {
        self.target()
    }

    /// A consultation sends the conversation to a second provider.
    pub fn needs_approval(&self) -> bool {
        self.cross_provider
    }

    /// Whether Zest may consult on its own (`[advisor] auto_review`).
    pub fn auto_review(&self) -> bool {
        self.advisor.auto_review
    }

    /// A consultation Zest starts itself, on the agent's behalf. Counts against
    /// the same per-request budget as the model's own calls. The caller checks
    /// consent first, because no approval card can be shown for it.
    pub async fn review(&self, messages: &[Message], question: &str) -> Result<String, String> {
        self.take_use(messages)?;
        let transcript = render_transcript(messages, self.advisor.transcript_budget());
        self.ask(&self.advisor.review_request(&transcript, question))
            .await
    }

    fn target(&self) -> String {
        format!("{} · {}", self.advisor.provider_id, self.advisor.model)
    }

    /// Count this consultation against the current request, or refuse it.
    fn take_use(&self, messages: &[Message]) -> Result<(), String> {
        let index = request_index(messages);
        let mut uses = self
            .uses
            .lock()
            .map_err(|_| "the advisor is unavailable".to_string())?;
        if uses.0 != index {
            *uses = (index, 0);
        }
        if uses.1 >= self.advisor.max_uses {
            return Err(format!(
                "The advisor has already been consulted {} time{} for this request. Continue with what you have.",
                self.advisor.max_uses,
                if self.advisor.max_uses == 1 { "" } else { "s" }
            ));
        }
        uses.1 += 1;
        Ok(())
    }

    async fn ask(&self, request: &TurnRequest) -> Result<String, String> {
        let task_id = crate::thread::new_id("task");
        if let Some(ledger) = &self.ledger {
            if let Ok(mut ledger) = ledger.lock() {
                ledger.begin_run_task(task_id.clone(), "advisor", None, None, None);
            }
        }
        let trace = TaskTraceGuard::new(self.ledger.clone(), task_id.clone());
        let started = tokio::time::Instant::now();
        let mut sink = |_event: StreamEvent<'_>| {};
        let result = tokio::time::timeout(
            self.advisor.timeout,
            self.advisor.provider.stream_turn(request, &mut sink),
        )
        .await;
        let elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        let completion = match &result {
            Ok(Ok(completion)) => Some(completion),
            _ => None,
        };
        self.record(&task_id, request, elapsed_ms, completion);

        let answer = match result {
            Err(_) => Err(format!(
                "The advisor did not answer within {} seconds. Continue without it.",
                self.advisor.timeout.as_secs()
            )),
            Ok(Err(error)) => Err(format!(
                "The advisor could not be reached: {error}. Continue without it."
            )),
            Ok(Ok(completion)) => {
                let text = advice_text(&completion);
                if text.trim().is_empty() {
                    Err("The advisor returned no advice. Continue without it.".to_string())
                } else {
                    Ok(text)
                }
            }
        };
        trace.finish(if answer.is_ok() {
            "completed"
        } else {
            "failed"
        });
        answer
    }

    /// Advisor spend is recorded under the advisor's own provider and model, so
    /// it shows up beside the working model's rather than inside it.
    fn record(
        &self,
        task_id: &str,
        request: &TurnRequest,
        elapsed_ms: u64,
        completion: Option<&Completion>,
    ) {
        let Some(ledger) = &self.ledger else {
            return;
        };
        let Ok(mut ledger) = ledger.lock() else {
            return;
        };
        let provider_id = self.advisor.provider_id.as_str();
        ledger.record_task_request(
            task_id,
            crate::agent::task_request_usage(
                "advisor",
                provider_id,
                request,
                elapsed_ms,
                completion,
            ),
        );
        if let Some(completion) = completion {
            ledger.record(
                provider_id,
                crate::agent::billed_model(&request.model, completion.served_model.as_deref()),
                completion,
            );
        }
    }
}

#[async_trait]
impl Tool for ConsultAdvisor {
    fn name(&self) -> &str {
        ADVISOR_TOOL
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "What you want advice on. Optional: without it the advisor reviews the approach so far."
                }
            },
            "additionalProperties": false
        })
    }

    fn risk(&self) -> ToolRisk {
        if self.cross_provider {
            ToolRisk::Exec
        } else {
            ToolRisk::Read
        }
    }

    fn uses_context(&self) -> bool {
        true
    }

    fn update_context(&self, messages: &[Message]) {
        if let Ok(mut context) = self.context.write() {
            *context = messages.to_vec();
        }
    }

    fn prepare(&self, input: Value) -> Result<PreparedToolCall, String> {
        let target = self.target();
        Ok(PreparedToolCall::plain_with_preview(
            ADVISOR_TOOL,
            self.risk(),
            input,
            ApprovalPreview {
                // Also the approval target, so "Allow for session" trusts this
                // provider and model only.
                summary: format!("Send this conversation to {target} for advice"),
                path: target,
                diff: String::new(),
            },
        ))
    }

    async fn run(&self, input: Value) -> Result<ToolOutcome, String> {
        let question = input
            .get("question")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let transcript = {
            let context = self
                .context
                .read()
                .map_err(|_| "the advisor is unavailable".to_string())?;
            self.take_use(&context)?;
            render_transcript(&context, self.advisor.transcript_budget())
        };
        let request = self.advisor.request(&transcript, question.as_deref());
        let advice = self.ask(&request).await?;
        let summary: String = advice.trim().chars().take(SUMMARY_CHARS).collect();
        Ok(ToolOutcome::text(format!(
            "Advice from {} (it saw this conversation up to your question):\n\n{}",
            self.advisor.model,
            advice.trim()
        ))
        .with_summary(summary))
    }
}

fn advice_text(completion: &Completion) -> String {
    completion
        .content
        .iter()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::auth::AuthStatus;
    use crate::error::{HarnessError, Result};
    use crate::provider::Provider;
    use crate::Usage;

    #[derive(Default)]
    struct Recording {
        requests: Mutex<Vec<TurnRequest>>,
        fail: bool,
        hang: bool,
    }

    #[async_trait]
    impl Provider for Recording {
        fn id(&self) -> &str {
            "strong"
        }
        fn default_model(&self) -> &str {
            "strong-model"
        }
        fn auth_status(&self) -> AuthStatus {
            AuthStatus::Ready { account: None }
        }
        async fn stream_turn(
            &self,
            request: &TurnRequest,
            _sink: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
        ) -> Result<Completion> {
            self.requests.lock().unwrap().push(request.clone());
            if self.hang {
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            if self.fail {
                return Err(HarnessError::Other("overloaded".into()));
            }
            Ok(Completion {
                content: vec![json!({"type":"text","text":"Mock the clock instead."})],
                stop_reason: Some("end_turn".into()),
                usage: Usage {
                    input_tokens: 900,
                    output_tokens: 40,
                    ..Usage::default()
                },
                usage_available: true,
                served_model: None,
                limits: None,
                provider_session: None,
            })
        }
    }

    fn advisor(provider: Arc<Recording>, max_uses: u32) -> Advisor {
        Advisor {
            provider_id: "strong".into(),
            model: "strong-model".into(),
            effort: None,
            provider,
            max_uses,
            max_tokens: 4_096,
            timeout: Duration::from_millis(200),
            auto_review: true,
        }
    }

    fn conversation(request: &str) -> Vec<Message> {
        vec![
            Message::user_text(request),
            Message::assistant(vec![
                json!({"type":"text","text":"Checking with the advisor."}),
                json!({"type":"tool_use","id":"c1","name":ADVISOR_TOOL,"input":{}}),
            ]),
        ]
    }

    #[tokio::test]
    async fn a_consultation_sends_the_quoted_conversation_with_no_tools() {
        let provider = Arc::new(Recording::default());
        let ledger = Arc::new(Mutex::new(Ledger::default()));
        let tool = ConsultAdvisor::new(advisor(provider.clone(), 3), "cheap", Some(ledger.clone()));
        tool.update_context(&conversation("Fix the flaky timer test"));

        let outcome = tool
            .run(json!({"question": "Mock the clock or add retries?"}))
            .await
            .expect("advice");

        assert!(
            outcome.body.contains("Mock the clock instead."),
            "{}",
            outcome.body
        );
        assert_eq!(outcome.summary.as_deref(), Some("Mock the clock instead."));
        let requests = provider.requests.lock().unwrap();
        let request = &requests[0];
        assert!(request.tools.is_empty() && !request.allow_tool_use);
        assert_eq!(request.messages.len(), 1);
        let prompt = request.messages[0].content[0]["text"].as_str().unwrap();
        assert!(prompt.contains("Fix the flaky timer test"), "{prompt}");
        assert!(
            prompt.contains("Question: Mock the clock or add retries?"),
            "{prompt}"
        );
        assert!(!prompt.contains("tool_use"), "{prompt}");
        assert!(request
            .system
            .as_ref()
            .is_some_and(|system| system.cacheable.contains("advisor to a coding agent")));
        assert!(
            ledger.lock().unwrap().get("strong").is_some(),
            "advisor spend must be recorded under its own provider"
        );
    }

    #[tokio::test]
    async fn the_limit_applies_per_request_and_resets_on_the_next_one() {
        let provider = Arc::new(Recording::default());
        let tool = ConsultAdvisor::new(advisor(provider, 1), "cheap", None);
        tool.update_context(&conversation("first request"));
        tool.run(json!({})).await.expect("first consultation");
        let refused = tool.run(json!({})).await.unwrap_err();
        assert!(
            refused.contains("already been consulted 1 time"),
            "{refused}"
        );

        let mut next = conversation("first request");
        next.push(Message::user_text("second request"));
        tool.update_context(&next);
        tool.run(json!({}))
            .await
            .expect("a new request gets a new allowance");
    }

    #[tokio::test]
    async fn failures_become_tool_errors_the_model_can_continue_past() {
        let failing = Arc::new(Recording {
            fail: true,
            ..Recording::default()
        });
        let tool = ConsultAdvisor::new(advisor(failing, 3), "cheap", None);
        tool.update_context(&conversation("go"));
        let error = tool.run(json!({})).await.unwrap_err();
        assert!(
            error.contains("could not be reached") && error.contains("Continue without it"),
            "{error}"
        );

        let slow = Arc::new(Recording {
            hang: true,
            ..Recording::default()
        });
        let tool = ConsultAdvisor::new(advisor(slow, 3), "cheap", None);
        tool.update_context(&conversation("go"));
        let error = tool.run(json!({})).await.unwrap_err();
        assert!(error.contains("did not answer within"), "{error}");
    }

    #[test]
    fn a_second_provider_needs_approval_for_that_provider_only() {
        let provider = Arc::new(Recording::default());
        let same = ConsultAdvisor::new(advisor(provider.clone(), 3), "strong", None);
        assert_eq!(same.risk(), ToolRisk::Read);

        let other = ConsultAdvisor::new(advisor(provider, 3), "cheap", None);
        assert_eq!(other.risk(), ToolRisk::Exec);
        let prepared = other.prepare(json!({})).unwrap();
        assert_eq!(prepared.risk, ToolRisk::Exec);
        assert_eq!(prepared.preview.path, "strong · strong-model");
        assert!(prepared.preview.summary.contains("Send this conversation"));
    }
}
