//! Ephemeral questions sharing a frozen prompt prefix with the parent chat.
//! No ThreadStore, tool execution, or parent mutations belong in this path.

use std::sync::{Arc, Mutex};

use crate::anthropic::types::Message;
use crate::cancel::CancelToken;
use crate::error::{HarnessError, Result};
use crate::provider::{Provider, StreamEvent, TurnRequest};
use crate::usage::Ledger;

#[derive(Clone)]
pub struct SideConversation {
    provider: Arc<dyn Provider>,
    ledger: Option<Arc<Mutex<Ledger>>>,
    request: TurnRequest,
}

impl SideConversation {
    pub fn fork_snapshot(&self) -> Self {
        let mut snapshot = self.clone();
        if let Some(provider) = self.provider.side_conversation_provider() {
            snapshot.provider = provider;
        }
        snapshot
    }

    pub(crate) fn new(
        provider: Arc<dyn Provider>,
        ledger: Option<Arc<Mutex<Ledger>>>,
        request: TurnRequest,
    ) -> Self {
        Self {
            provider,
            ledger,
            request,
        }
    }

    /// An in-flight parent cursor may already contain partial new work. Replay
    /// the frozen completed transcript instead of forking that moving target.
    pub fn without_provider_session(mut self) -> Self {
        self.request.provider_session = None;
        self
    }

    pub async fn send(
        &mut self,
        text: &str,
        cancel: &CancelToken,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    ) -> Result<String> {
        if text.trim().is_empty() {
            return Err(HarnessError::Other(
                "Write a question for the side conversation.".into(),
            ));
        }
        if cancel.is_cancelled() {
            return Err(HarnessError::Cancelled);
        }
        let mut request = self.request.clone();
        // Keep the entire existing prefix unchanged. Instructions specific to
        // this question come after it, preserving eligible provider caches.
        request.messages.push(Message::user_text(format!(
            "[Zest /btw: temporary side conversation. Answer the question using the conversation above. \
             Do not continue the main task, change files, or run tools. These messages will not be \
             added to the main conversation.]\n\n{}", text.trim()
        )));
        request.cancel = Some(cancel.clone());
        let completion = match self.provider.stream_turn(&request, on_event).await {
            Ok(completion) => completion,
            Err(error) => {
                // The failed provider session may contain a partial answer.
                // Retry from the last successful side transcript instead.
                self.request.provider_session = None;
                return Err(error);
            }
        };
        if let Some(ledger) = &self.ledger {
            if let Ok(mut ledger) = ledger.lock() {
                ledger.record(
                    self.provider.id(),
                    completion.served_model.as_deref().unwrap_or(&request.model),
                    &completion,
                );
            }
        }
        let answer = completion
            .content
            .iter()
            .filter_map(|block| {
                (block["type"] == "text")
                    .then(|| block["text"].as_str())
                    .flatten()
            })
            .collect::<Vec<_>>()
            .join("\n");
        if cancel.is_cancelled()
            || answer.trim().is_empty()
            || completion
                .content
                .iter()
                .any(|block| block["type"] == "tool_use")
        {
            self.request.provider_session = None;
            return Err(if cancel.is_cancelled() {
                HarnessError::Cancelled
            } else {
                HarnessError::Other(
                    "The provider did not return an answer to the side question. Try again.".into(),
                )
            });
        }
        request.messages.push(Message {
            role: "assistant".into(),
            content: completion.content,
        });
        request.provider_session = completion.provider_session;
        request.cancel = None;
        self.request = request;
        Ok(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Completion, ProviderSessionRef};
    use crate::tools::ToolRegistry;
    use crate::{Agent, AuthStatus, Usage};
    use async_trait::async_trait;
    use serde_json::json;

    #[derive(Default)]
    struct RecordingProvider {
        requests: Mutex<Vec<TurnRequest>>,
    }

    #[async_trait]
    impl Provider for RecordingProvider {
        fn id(&self) -> &str {
            "btw-test"
        }
        fn default_model(&self) -> &str {
            "test-model"
        }
        fn auth_status(&self) -> AuthStatus {
            AuthStatus::Ready { account: None }
        }
        async fn stream_turn(
            &self,
            request: &TurnRequest,
            sink: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
        ) -> Result<Completion> {
            self.requests.lock().unwrap().push(request.clone());
            let prompt = serde_json::to_string(request.messages.last().unwrap()).unwrap();
            if prompt.contains("fail-question") {
                return Err(HarnessError::PrematureEof);
            }
            if prompt.contains("cancel-question") {
                request.cancel.as_ref().unwrap().cancel();
            }
            sink(StreamEvent::Text("side answer"));
            Ok(Completion {
                content: if prompt.contains("tool-question") {
                    vec![
                        json!({"type":"tool_use", "id":"forbidden", "name":"write_file", "input":{}}),
                    ]
                } else {
                    vec![json!({"type":"text", "text":"side answer"})]
                },
                stop_reason: Some("end_turn".into()),
                usage: Usage {
                    input_tokens: 12,
                    output_tokens: 3,
                    ..Usage::default()
                },
                usage_available: true,
                served_model: None,
                limits: None,
                provider_session: Some(ProviderSessionRef::CodexAppServer {
                    thread_id: "child".into(),
                }),
            })
        }
    }

    fn parent(provider: Arc<RecordingProvider>) -> Agent {
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(crate::tools::AskUser));
        let mut agent = Agent::new(provider, tools).with_system("Stable system prompt");
        agent.messages = vec![
            Message::user_text("Implement the loader"),
            Message::assistant(vec![
                json!({"type":"tool_use", "id":"read-1", "name":"read_file", "input":{"path":"loader.rs"}}),
            ]),
            Message {
                role: "user".into(),
                content: vec![
                    json!({"type":"tool_result", "tool_use_id":"read-1", "content":"The loader uses a bounded queue."}),
                ],
            },
            Message::assistant(vec![json!({"type":"text", "text":"The queue is bounded."})]),
        ];
        agent.provider_session = Some(ProviderSessionRef::CodexAppServer {
            thread_id: "parent".into(),
        });
        agent.last_usage = Some(Usage {
            input_tokens: 99,
            ..Usage::default()
        });
        agent
    }

    #[tokio::test]
    async fn btw_preserves_parent_context_cursor_and_usage_across_followups() {
        let provider = Arc::new(RecordingProvider::default());
        let ledger = Arc::new(Mutex::new(Ledger::default()));
        let mut parent = parent(provider.clone()).with_ledger(ledger.clone());
        let original = serde_json::to_value(&parent.messages).unwrap();
        let cursor = parent.provider_session.clone();
        let mut side = parent.side_conversation();
        assert_eq!(
            side.send("Why a queue?", &CancelToken::new(), &mut |_| {})
                .await
                .unwrap(),
            "side answer"
        );
        side.send("Explain the tradeoff", &CancelToken::new(), &mut |_| {})
            .await
            .unwrap();
        assert_eq!(serde_json::to_value(&parent.messages).unwrap(), original);
        assert_eq!(parent.provider_session, cursor);
        assert_eq!(parent.last_usage.as_ref().unwrap().input_tokens, 99);
        assert!(parent.turn_usage.is_none());
        assert!(!ledger.lock().unwrap().snapshot().providers.is_empty());
        drop(side);
        parent
            .send("Continue the main task", &mut |_| {})
            .await
            .unwrap();
        let requests = provider.requests.lock().unwrap();
        assert_eq!(
            serde_json::to_value(&requests[0].messages[..4]).unwrap(),
            original
        );
        assert_eq!(
            requests[0].system.as_ref().unwrap().text(),
            parent.system_text()
        );
        assert_eq!(
            serde_json::to_value(&requests[0].tools).unwrap(),
            serde_json::to_value(&requests[2].tools).unwrap()
        );
        assert_eq!(requests[0].tools.len(), 1);
        assert_eq!(requests[0].tools[0].name, "ask_user");
        assert!(!requests[0].allow_tool_use);
        assert!(requests[0].interaction.is_none());
        assert_eq!(
            requests[0].provider_session,
            Some(ProviderSessionRef::CodexAppServerFork {
                thread_id: "parent".into()
            })
        );
        assert_eq!(
            requests[1].provider_session,
            Some(ProviderSessionRef::CodexAppServer {
                thread_id: "child".into()
            })
        );
        assert!(serde_json::to_string(&requests[1].messages)
            .unwrap()
            .contains("Why a queue?"));
        assert_eq!(requests[2].provider_session, cursor);
        assert!(!serde_json::to_string(&requests[2].messages)
            .unwrap()
            .contains("Why a queue?"));
    }

    #[tokio::test]
    async fn active_side_context_publishes_the_submitted_prompt_before_completion() {
        let provider = Arc::new(RecordingProvider::default());
        let mut parent = parent(provider);
        let mut snapshots = Vec::new();
        let mut sink = |_event: StreamEvent<'_>| {};
        let mut publish = |snapshot: SideConversation| snapshots.push(snapshot);

        parent
            .send_cancellable_with_inbox_and_side_context(
                "Current main prompt",
                &mut sink,
                None,
                None,
                &mut publish,
            )
            .await
            .unwrap();

        assert_eq!(snapshots.len(), 2);
        assert!(snapshots[0]
            .request
            .messages
            .iter()
            .any(|message| serde_json::to_string(message)
                .unwrap()
                .contains("Current main prompt")));
        assert!(snapshots[0].request.provider_session.is_none());
        assert!(serde_json::to_string(&snapshots[1].request.messages)
            .unwrap()
            .contains("side answer"));
    }

    #[tokio::test]
    async fn btw_failure_cancellation_and_unexpected_tools_roll_back_the_side_turn() {
        for question in ["fail-question", "cancel-question", "tool-question"] {
            let provider = Arc::new(RecordingProvider::default());
            let parent = parent(provider.clone());
            let mut side = parent.side_conversation();
            let messages = serde_json::to_value(&side.request.messages).unwrap();
            assert!(side
                .send(question, &CancelToken::new(), &mut |_| {})
                .await
                .is_err());
            assert_eq!(
                serde_json::to_value(&side.request.messages).unwrap(),
                messages
            );
            assert!(side.request.provider_session.is_none());
            side.send("Retry", &CancelToken::new(), &mut |_| {})
                .await
                .unwrap();
            let requests = provider.requests.lock().unwrap();
            assert!(requests[1].provider_session.is_none());
            assert!(!serde_json::to_string(&requests[1].messages)
                .unwrap()
                .contains(question));
        }
    }

    #[tokio::test]
    async fn btw_empty_or_precancelled_questions_do_not_call_the_provider() {
        let provider = Arc::new(RecordingProvider::default());
        let mut side = parent(provider.clone()).side_conversation();
        assert!(side
            .send("  ", &CancelToken::new(), &mut |_| {})
            .await
            .is_err());
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(side.send("Question", &cancel, &mut |_| {}).await.is_err());
        assert!(provider.requests.lock().unwrap().is_empty());
    }
}
