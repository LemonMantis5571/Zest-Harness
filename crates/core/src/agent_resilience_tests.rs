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
}

impl ScriptedProvider {
    fn new(script: Vec<Result<Completion>>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            requests: Mutex::new(Vec::new()),
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
