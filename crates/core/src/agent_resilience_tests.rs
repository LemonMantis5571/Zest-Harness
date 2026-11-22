//! How a turn survives provider hiccups: interrupted turns keep their history,
//! malformed tool input goes back to the model, truncated answers continue,
//! runaway loops stop, and an over-long context compacts mid-turn.

use super::*;
use crate::anthropic::types::Usage;
use crate::auth::AuthStatus;
use crate::provider::Completion;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::VecDeque;

/// Answers each `stream_turn` with the next scripted outcome and records the
/// messages every request carried.
#[derive(Default)]
struct ScriptedProvider {
    script: Mutex<VecDeque<Result<Completion>>>,
    requests: Mutex<Vec<TurnRequest>>,
    /// Context window to advertise; the static table's default when `None`.
    window: Option<u64>,
}

impl ScriptedProvider {
    fn new(script: Vec<Result<Completion>>) -> Arc<Self> {
        Self::with_window(script, None)
    }

    fn with_window(script: Vec<Result<Completion>>, window: Option<u64>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            requests: Mutex::new(Vec::new()),
            window,
        })
    }

    fn requests(&self) -> Vec<TurnRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn id(&self) -> &str {
        "scripted"
    }
    fn default_model(&self) -> &str {
        "scripted-model"
    }
    fn auth_status(&self) -> AuthStatus {
        AuthStatus::Ready { account: None }
    }
    fn models(&self) -> Vec<crate::provider::ModelSpec> {
        self.window
            .map(|context_window| crate::provider::ModelSpec {
                id: self.default_model().into(),
                efforts: Vec::new(),
                context_window,
                supports_tools: true,
                supports_vision: false,
                resolves_to: None,
                needs_cli: None,
            })
            .into_iter()
            .collect()
    }
    async fn stream_turn(
        &self,
        request: &TurnRequest,
        _on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    ) -> Result<Completion> {
        self.requests.lock().unwrap().push(request.clone());
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(HarnessError::Other("script exhausted".into())))
    }
}

fn reply(content: Vec<Value>, stop: &str) -> Result<Completion> {
    Ok(Completion {
        content,
        stop_reason: Some(stop.into()),
        usage: Usage::default(),
        usage_available: true,
        limits: None,
        served_model: None,
        provider_session: None,
    })
}

fn text(body: &str) -> Value {
    json!({ "type": "text", "text": body })
}

fn tool_call(id: &str, name: &str, input: Value) -> Value {
    json!({ "type": "tool_use", "id": id, "name": name, "input": input })
}

/// Echoes its input back, and optionally cancels the turn while it runs.
struct EchoTool {
    cancel: Option<CancelToken>,
}

#[async_trait]
impl crate::tools::Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "echo the input"
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "n": { "type": "integer" } } })
    }
    async fn run(&self, input: Value) -> std::result::Result<crate::tools::ToolOutcome, String> {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
        Ok(crate::tools::ToolOutcome::text(format!("echoed {input}")))
    }
}

fn agent_with_echo(provider: Arc<ScriptedProvider>, cancel: Option<CancelToken>) -> Agent {
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(EchoTool { cancel }));
    Agent::new(provider, tools)
}

fn roles(messages: &[Message]) -> Vec<&str> {
    messages
        .iter()
        .map(|message| message.role.as_str())
        .collect()
}

fn wire(message: &Message) -> String {
    serde_json::to_string(&message.content).unwrap()
}

#[tokio::test]
async fn provider_error_commits_the_user_message_marked_interrupted() {
    let provider = ScriptedProvider::new(vec![Err(HarnessError::Other("provider boom".into()))]);
    let mut agent = agent_with_echo(provider, None);
    agent.messages.push(Message::user_text("prior"));
    agent
        .messages
        .push(Message::assistant(vec![text("prior answer")]));
    let mut sink = |_ev: StreamEvent<'_>| {};

    let error = agent.send("new", &mut sink).await.unwrap_err();

    assert!(matches!(error, HarnessError::Other(_)));
    assert_eq!(
        roles(&agent.messages),
        ["user", "assistant", "user", "user"]
    );
    assert!(wire(&agent.messages[2]).contains("new"));
    assert!(wire(&agent.messages[3]).contains("interrupted"));
}

#[tokio::test]
async fn a_failure_after_a_tool_round_keeps_the_calls_and_results() {
    let provider = ScriptedProvider::new(vec![
        reply(
            vec![
                text("Running it."),
                tool_call("call-1", "echo", json!({ "n": 1 })),
            ],
            "tool_use",
        ),
        Err(HarnessError::PrematureEof),
    ]);
    let mut agent = agent_with_echo(provider.clone(), None);
    let mut sink = |_ev: StreamEvent<'_>| {};

    agent.send("edit the file", &mut sink).await.unwrap_err();

    assert_eq!(
        roles(&agent.messages),
        ["user", "assistant", "user", "user"]
    );
    assert!(wire(&agent.messages[1]).contains("call-1"));
    let result = &agent.messages[2].content[0];
    assert_eq!(result["type"], "tool_result");
    assert_eq!(result["tool_use_id"], "call-1");
    assert!(result["content"].to_string().contains("echoed"));
    assert!(wire(&agent.messages[3]).contains("interrupted"));

    // The next turn sees what the interrupted one did.
    let provider_retry = ScriptedProvider::new(vec![reply(vec![text("ok")], "end_turn")]);
    agent.provider = provider_retry.clone();
    agent.send("go on", &mut sink).await.unwrap();
    let seen = &provider_retry.requests()[0].messages;
    assert!(seen.iter().any(|message| wire(message).contains("echoed")));
}

#[tokio::test]
async fn cancelling_during_a_tool_keeps_its_result_and_closes_the_turn() {
    let cancel = CancelToken::new();
    let provider = ScriptedProvider::new(vec![reply(
        vec![tool_call("call-1", "echo", json!({}))],
        "tool_use",
    )]);
    let mut agent = agent_with_echo(provider.clone(), Some(cancel.clone()));
    let mut sink = |_ev: StreamEvent<'_>| {};

    let error = agent
        .send_cancellable("edit", &mut sink, Some(&cancel))
        .await
        .unwrap_err();

    assert!(matches!(error, HarnessError::Cancelled));
    assert_eq!(provider.requests().len(), 1);
    assert_eq!(
        roles(&agent.messages),
        ["user", "assistant", "user", "user"]
    );
    assert!(wire(&agent.messages[2]).contains("echoed"));
    assert!(wire(&agent.messages[3]).contains("stopped it"));
}

#[tokio::test]
async fn an_unanswered_tool_call_is_closed_when_the_turn_stops() {
    let mut agent = agent_with_echo(ScriptedProvider::new(Vec::new()), None);
    let progress = TurnProgress {
        staged: vec![
            Message::user_text("go"),
            Message::assistant(vec![tool_call("call-9", "echo", json!({}))]),
        ],
        sensitive: Vec::new(),
        last_usage: None,
    };

    agent.commit_interrupted_turn(progress, &HarnessError::Cancelled);

    assert_eq!(
        roles(&agent.messages),
        ["user", "assistant", "user", "user"]
    );
    let closed = &agent.messages[2].content[0];
    assert_eq!(closed["tool_use_id"], "call-9");
    assert_eq!(closed["is_error"], true);
}

#[tokio::test]
async fn a_response_cut_off_mid_stream_is_continued() {
    let provider = ScriptedProvider::new(vec![
        reply(
            vec![text("First half")],
            crate::provider::STREAM_INTERRUPTED,
        ),
        reply(vec![text(" and the rest.")], "end_turn"),
    ]);
    let mut agent = agent_with_echo(provider.clone(), None);
    let mut sink = |_ev: StreamEvent<'_>| {};

    agent.send("explain", &mut sink).await.unwrap();

    let second = &provider.requests()[1].messages;
    assert!(wire(&second[1]).contains("First half"));
    assert!(wire(&second[2]).contains("cut off"));
    assert_eq!(
        roles(&agent.messages),
        ["user", "assistant", "user", "assistant"]
    );
}

#[tokio::test]
async fn a_response_that_keeps_getting_cut_off_stops_the_turn() {
    let provider = ScriptedProvider::new(
        (0..=MAX_CONTINUATIONS)
            .map(|_| reply(vec![text("again")], crate::provider::STREAM_INTERRUPTED))
            .collect(),
    );
    let mut agent = agent_with_echo(provider.clone(), None);
    let mut sink = |_ev: StreamEvent<'_>| {};

    let error = agent.send("explain", &mut sink).await.unwrap_err();

    assert!(matches!(error, HarnessError::StoppedEarly(_)));
    assert_eq!(provider.requests().len() as u32, MAX_CONTINUATIONS + 1);
}

#[tokio::test]
async fn malformed_tool_arguments_go_back_to_the_model_as_a_tool_error() {
    let provider = ScriptedProvider::new(vec![
        reply(
            vec![
                tool_call("bad", "echo", json!("{\"n\": ")),
                tool_call("good", "echo", json!("{\"n\": \"2\"}")),
            ],
            "tool_use",
        ),
        reply(vec![text("Retried the call.")], "end_turn"),
    ]);
    let mut agent = agent_with_echo(provider.clone(), None);
    let mut sink = |_ev: StreamEvent<'_>| {};

    agent.send("go", &mut sink).await.unwrap();

    let second = &provider.requests()[1].messages;
    // The echoed call carries an object, never the broken text.
    assert_eq!(second[1].content[0]["input"], json!({}));
    let results = &second[2].content;
    assert_eq!(results[0]["tool_use_id"], "bad");
    assert_eq!(results[0]["is_error"], true);
    assert!(results[0]["content"]
        .to_string()
        .contains("Call the tool again"));
    // The repaired call ran with its number unquoted.
    assert_eq!(results[1]["is_error"], false);
    assert!(results[1]["content"]
        .to_string()
        .contains("echoed {\\\"n\\\":2}"));
}

#[tokio::test]
async fn a_response_cut_off_at_max_tokens_is_continued_in_smaller_pieces() {
    let provider = ScriptedProvider::new(vec![
        reply(vec![text("Part one")], "max_tokens"),
        reply(vec![text("Part two.")], "end_turn"),
    ]);
    let mut agent = agent_with_echo(provider.clone(), None);
    let mut sink = |_ev: StreamEvent<'_>| {};

    agent.send("write it", &mut sink).await.unwrap();

    let second = &provider.requests()[1].messages;
    assert!(wire(&second[1]).contains("Part one"));
    assert!(wire(&second[2]).contains("smaller"));
}

#[tokio::test]
async fn max_tokens_keeps_finished_calls_and_rejects_the_cut_off_one() {
    let provider = ScriptedProvider::new(vec![
        reply(
            vec![
                tool_call("done", "echo", json!({ "n": 1 })),
                tool_call("cut", "echo", json!("{\"n\": 2, \"te")),
            ],
            "max_tokens",
        ),
        reply(vec![text("Finished.")], "end_turn"),
    ]);
    let mut agent = agent_with_echo(provider.clone(), None);
    let mut sink = |_ev: StreamEvent<'_>| {};

    agent.send("go", &mut sink).await.unwrap();

    let second = &provider.requests()[1].messages;
    let results = &second[2].content;
    assert_eq!(results[0]["is_error"], false);
    assert_eq!(results[1]["is_error"], true);
    assert!(wire(&second[3]).contains("output token limit"));
}

#[tokio::test]
async fn an_empty_response_is_asked_for_again() {
    let provider = ScriptedProvider::new(vec![
        reply(Vec::new(), "end_turn"),
        reply(vec![text("Here it is.")], "end_turn"),
    ]);
    let mut agent = agent_with_echo(provider.clone(), None);
    let mut sink = |_ev: StreamEvent<'_>| {};

    agent.send("answer", &mut sink).await.unwrap();

    let second = &provider.requests()[1].messages;
    assert_eq!(roles(second), ["user", "user"]);
    assert!(wire(&second[1]).contains("empty"));
}

#[tokio::test]
async fn an_unknown_stop_reason_is_continued_then_gives_up() {
    let provider = ScriptedProvider::new(
        (0..=MAX_CONTINUATIONS)
            .map(|_| reply(vec![text("partial")], "model_context_window_exceeded_v9"))
            .collect(),
    );
    let mut agent = agent_with_echo(provider.clone(), None);
    let mut sink = |_ev: StreamEvent<'_>| {};

    let error = agent.send("go", &mut sink).await.unwrap_err();

    assert!(matches!(error, HarnessError::StoppedEarly(_)));
    assert_eq!(provider.requests().len() as u32, MAX_CONTINUATIONS + 1);
    assert!(wire(&provider.requests()[1].messages[2]).contains("stopped unexpectedly"));
}

#[tokio::test]
async fn the_step_limit_ends_with_a_round_that_may_not_call_tools() {
    let provider = ScriptedProvider::new(vec![
        reply(vec![tool_call("a", "echo", json!({ "n": 1 }))], "tool_use"),
        reply(vec![tool_call("b", "echo", json!({ "n": 2 }))], "tool_use"),
        reply(
            vec![
                text("Done so far: two echoes."),
                tool_call("c", "echo", json!({ "n": 3 })),
            ],
            "tool_use",
        ),
    ]);
    let mut agent = agent_with_echo(provider.clone(), None);
    agent.max_steps = 2;
    let mut sink = |_ev: StreamEvent<'_>| {};

    agent.send("loop", &mut sink).await.unwrap();

    let requests = provider.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[0].allow_tool_use && requests[1].allow_tool_use);
    assert!(!requests[2].allow_tool_use);
    assert!(wire(requests[2].messages.last().unwrap()).contains("limit of 2 steps"));
    // The wrap-up's tool call is dropped, so history stays valid.
    let last = agent.messages.last().unwrap();
    assert_eq!(last.role, "assistant");
    assert!(!wire(last).contains("tool_use"));
}

fn same_call(id: &str) -> Result<Completion> {
    reply(vec![tool_call(id, "echo", json!({ "n": 7 }))], "tool_use")
}

#[tokio::test]
async fn the_same_call_three_times_stops_a_headless_turn() {
    let provider = ScriptedProvider::new(vec![same_call("1"), same_call("2"), same_call("3")]);
    let mut agent = agent_with_echo(provider.clone(), None);
    let mut questions = 0;
    let mut sink = |event: StreamEvent<'_>| {
        if matches!(event, StreamEvent::QuestionNeeded { .. }) {
            questions += 1;
        }
    };

    let error = agent.send("loop", &mut sink).await.unwrap_err();

    assert!(matches!(error, HarnessError::StoppedEarly(_)));
    assert_eq!(questions, 1);
    // The third call never ran; its committed result says so.
    let results = wire(&agent.messages[agent.messages.len() - 2]);
    assert!(results.contains("Not run"), "{results}");
}

struct Answer(&'static str);

#[async_trait]
impl crate::tools::question::Questioner for Answer {
    async fn answer(
        &self,
        _request: &crate::tools::QuestionRequest,
    ) -> std::result::Result<String, String> {
        Ok(self.0.into())
    }
}

#[tokio::test]
async fn the_user_can_let_a_repeating_model_keep_going() {
    let provider = ScriptedProvider::new(vec![
        same_call("1"),
        same_call("2"),
        same_call("3"),
        reply(vec![text("Finally done.")], "end_turn"),
    ]);
    let mut agent =
        agent_with_echo(provider.clone(), None).with_questioner(Arc::new(Answer("Keep going")));
    let mut sink = |_ev: StreamEvent<'_>| {};

    agent.send("loop", &mut sink).await.unwrap();

    assert_eq!(provider.requests().len(), 4);
}

#[test]
fn a_different_input_resets_the_repeat_count() {
    let call = |n: i64| crate::anthropic::types::ToolUse {
        id: format!("id-{n}"),
        name: "echo".into(),
        input: json!({ "n": n }),
    };
    let mut tracker = RepeatTracker::default();
    let (one, two) = (call(1), call(2));
    // A batch repeating one call counts once.
    assert!(tracker.observe([&one, &one, &one]).is_none());
    assert!(tracker.observe([&one]).is_none());
    assert!(tracker.observe([&two]).is_none());
    assert!(tracker.observe([&two]).is_none());
    assert!(tracker.observe([&two]).is_some());
    // Counting starts over after the user is asked.
    assert!(tracker.observe([&two]).is_none());
    assert!(tracker.observe([&two]).is_none());
    assert!(tracker.observe([&two]).is_some());
}

/// Four messages of about 2,500 tokens each, with no tool result to trim.
fn long_history() -> Vec<Message> {
    let long = "word ".repeat(2_000);
    vec![
        Message::user_text(&long),
        Message::assistant(vec![text(&long)]),
        Message::user_text(&long),
        Message::assistant(vec![text(&long)]),
    ]
}

#[tokio::test]
async fn a_turn_near_the_window_compacts_before_its_step() {
    let provider = ScriptedProvider::with_window(
        vec![
            reply(vec![text("CHECKPOINT: earlier work")], "end_turn"),
            reply(vec![text("Answer.")], "end_turn"),
        ],
        Some(12_000),
    );
    let mut agent = agent_with_echo(provider.clone(), None).with_messages(long_history());
    let mut activity = Vec::new();
    let mut sink = |event: StreamEvent<'_>| {
        if let StreamEvent::ProviderActivity { title, status, .. } = event {
            activity.push(format!("{status}: {title}"));
        }
    };

    agent.send("next request", &mut sink).await.unwrap();

    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        !requests[0].allow_tool_use,
        "the first request is the summary"
    );
    let step = &requests[1].messages;
    assert!(wire(&step[0]).contains("context checkpoint"));
    assert!(wire(&step[1]).contains("CHECKPOINT: earlier work"));
    assert!(
        wire(&step[2]).contains("next request"),
        "kept word for word"
    );
    assert!(wire(&step[3]).contains("compacted"));
    assert!(activity
        .iter()
        .any(|line| line.starts_with("completed: Compacted")));
    // The committed history is the compacted one.
    assert!(wire(&agent.messages[0]).contains("context checkpoint"));
}

#[tokio::test]
async fn a_prompt_the_provider_rejects_as_too_long_is_compacted_and_retried() {
    let provider = ScriptedProvider::new(vec![
        Err(HarnessError::Api {
            status: 400,
            body: r#"{"error":{"message":"prompt is too long: 250000 tokens > 200000 maximum"}}"#
                .into(),
        }),
        reply(vec![text("CHECKPOINT")], "end_turn"),
        reply(vec![text("Answer.")], "end_turn"),
    ]);
    let mut agent = agent_with_echo(provider.clone(), None).with_messages(long_history());
    let mut sink = |_ev: StreamEvent<'_>| {};

    agent.send("next request", &mut sink).await.unwrap();

    let requests = provider.requests();
    assert_eq!(requests.len(), 3);
    assert!(!requests[1].allow_tool_use);
    assert!(wire(&requests[2].messages[1]).contains("CHECKPOINT"));
}

#[tokio::test]
async fn a_second_too_long_rejection_ends_the_turn() {
    let too_long = || {
        Err(HarnessError::Api {
            status: 413,
            body: String::new(),
        })
    };
    let provider = ScriptedProvider::new(vec![
        too_long(),
        reply(vec![text("CHECKPOINT")], "end_turn"),
        too_long(),
    ]);
    let mut agent = agent_with_echo(provider.clone(), None).with_messages(long_history());
    let mut sink = |_ev: StreamEvent<'_>| {};

    let error = agent.send("next request", &mut sink).await.unwrap_err();

    assert!(error.is_context_limit());
    assert_eq!(provider.requests().len(), 3);
}

#[tokio::test]
async fn a_cancelled_compaction_leaves_history_alone() {
    let provider = ScriptedProvider::new(vec![reply(vec![text("CHECKPOINT")], "end_turn")]);
    let mut agent = agent_with_echo(provider, None).with_messages(long_history());
    let cancel = CancelToken::new();
    cancel.cancel();

    let error = agent
        .compact_context_for_run(None, Some(&cancel))
        .await
        .unwrap_err();

    assert!(matches!(error, HarnessError::Cancelled));
    assert_eq!(agent.messages.len(), 4);
}

#[test]
fn a_history_too_long_to_summarize_loses_its_oldest_exchanges_first() {
    let long = "word ".repeat(2_000);
    let history = vec![
        Message::user_text("first request"),
        Message::assistant(vec![tool_call("t1", "echo", json!({}))]),
        Message::user_blocks(vec![crate::anthropic::types::tool_result(
            "t1", &long, false,
        )]),
        Message::assistant(vec![text(&long)]),
        Message::user_text("second request"),
        Message::assistant(vec![text("short answer")]),
    ];

    let fitted = fit_history_for_checkpoint(history, 500);

    assert!(wire(&fitted[0]).contains("left out"));
    assert!(wire(&fitted[1]).contains("second request"));
    assert_eq!(fitted.len(), 3);
}
