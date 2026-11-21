//! Experiment 3: a Tokio runtime on its own thread, events streamed into GPUI, actions sent back.
//!
//! Pattern: the runtime is owned by a plain `std::thread`, never by GPUI. Commands go in through a
//! `tokio::sync::mpsc` unbounded sender (sync `send`, callable from the GPUI thread). Events come
//! back through a `futures::channel::mpsc` unbounded sender, whose waker is GPUI's foreground
//! executor: awaiting the receiver inside `cx.spawn` is the whole "wake the UI" mechanism.

use futures::channel::mpsc as fmpsc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub enum Cmd {
    Run(String),
    Compact,
    /// A user turn: stream a canned reply for `secs` at `rate` tokens/s (with tool rows when `agent`).
    Reply { prompt: String, images: usize, agent: bool },
    Stream { rate: u32, secs: u32, chars: usize },
    /// An agent-shaped turn: reasoning, `tools` tool calls, then the streamed answer.
    Agent { rate: u32, secs: u32, chars: usize, tools: usize },
    Cancel,
}

pub struct Ev {
    /// Stamped on the Tokio thread, compared on the UI thread: end-to-end wake-up latency.
    pub at: Instant,
    pub kind: EvKind,
}

pub enum EvKind {
    Notice(String),
    Failure(String),
    SessionInfo(String),
    Activity { id: String, title: String, status: String },
    Card(crate::interaction::Card),
    Start,
    Thinking(String),
    Text(String),
    ToolStart { id: String, name: String },
    ToolResult { id: String, summary: String, diff: Option<String>, is_error: bool, artifact: Option<Arc<zest_core::html::HtmlDocument>> },
    End { tokens: usize, cancelled: bool },
}

pub struct Bridge {
    pub tx: tokio::sync::mpsc::UnboundedSender<Cmd>,
    pub rx: Option<fmpsc::UnboundedReceiver<Ev>>,
}

pub enum Piece<'a> {
    Think(&'a str),
    Text(&'a str),
    ToolStart { name: &'a str, id: &'a str },
    ToolResult { name: &'a str, id: &'a str, summary: &'a str, diff: Option<&'a str>, is_error: bool },
}

/// What one mock turn does: reasoning tokens, tool calls, then the answer tokens.
pub struct Script {
    pub think: Vec<String>,
    pub tools: Vec<crate::seed::ToolSeed>,
    pub tokens: Vec<String>,
}

impl Script {
    pub fn plain(tokens: Vec<String>) -> Self {
        let think = ["Looking ", "at ", "the ", "request, ", "then ", "drafting ", "an ", "answer."];
        Script { think: think.iter().map(|s| s.to_string()).collect(), tools: vec![], tokens }
    }

    pub fn agent(text: &str, tools: usize, salt: usize) -> Self {
        Script {
            think: crate::seed::tokenize(&crate::seed::think_text(salt)),
            tools: (0..tools).map(|k| crate::seed::tool_seed(salt + k)).collect(),
            tokens: crate::seed::tokenize(text),
        }
    }
}

/// Emits the script at roughly `rate` answer tokens per second, catching up after slow timer ticks.
pub async fn pace(
    script: &Script,
    rate: u32,
    max_secs: u32,
    cancel: &AtomicBool,
    emit: &mut (dyn FnMut(Piece) + Send),
) -> (usize, bool) {
    let tokens = &script.tokens;
    let slow = script.think.len() <= 16;
    for (n, t) in script.think.iter().enumerate() {
        emit(Piece::Think(t));
        if slow || n % 8 == 7 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    for (k, t) in script.tools.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return (0, true);
        }
        let id = format!("call_{k}");
        emit(Piece::ToolStart { name: t.name, id: &id });
        tokio::time::sleep(Duration::from_millis(t.delay_ms)).await;
        emit(Piece::ToolResult {
            name: t.name,
            id: &id,
            summary: &t.summary,
            diff: t.diff.as_deref(),
            is_error: t.is_error,
        });
    }
    let start = Instant::now();
    let mut sent = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return (sent, true);
        }
        let due = ((start.elapsed().as_secs_f64() * rate as f64) as usize).min(tokens.len());
        while sent < due {
            emit(Piece::Text(&tokens[sent]));
            sent += 1;
        }
        if sent >= tokens.len() || start.elapsed().as_secs() >= max_secs as u64 {
            return (sent, false);
        }
        tokio::time::sleep(Duration::from_millis(4)).await;
    }
}

#[cfg(feature = "core")]
mod mock {
    //! The same pacing, but surfaced through zest-core's `Provider::stream_turn` and its
    //! borrowed `StreamEvent<'a>` callback, i.e. the real provider-neutral streaming contract.
    use super::*;
    use zest_core::auth::AuthStatus;
    use zest_core::provider::{Completion, Provider, StreamEvent, TurnRequest};

    pub struct MockProvider {
        pub script: Arc<Script>,
        pub rate: u32,
        pub secs: u32,
        pub cancel: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl Provider for MockProvider {
        fn id(&self) -> &str {
            "mock"
        }
        fn default_model(&self) -> &str {
            "mock-1"
        }
        fn auth_status(&self) -> AuthStatus {
            AuthStatus::Ready { account: None }
        }
        async fn stream_turn(
            &self,
            _req: &TurnRequest,
            on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
        ) -> zest_core::error::Result<Completion> {
            let (sent, _) = pace(&self.script, self.rate, self.secs, &self.cancel, &mut |p| match p {
                Piece::Think(t) => on_event(StreamEvent::Thinking(t)),
                Piece::Text(t) => on_event(StreamEvent::Text(t)),
                Piece::ToolStart { name, id } => on_event(StreamEvent::ToolCallStart { name, id }),
                Piece::ToolResult { name, id, summary, diff, is_error } => {
                    on_event(StreamEvent::ToolCallResult {
                        name,
                        id,
                        summary,
                        is_error,
                        path: None,
                        diff,
                        metadata: None,
                    })
                }
            })
            .await;
            Ok(Completion {
                content: vec![serde_json::json!({"type": "text", "text": format!("<{sent} tokens>")})],
                stop_reason: Some("end_turn".into()),
                usage: Default::default(),
                usage_available: false,
                limits: None,
                served_model: None,
                provider_session: None,
            })
        }
    }

    pub async fn turn(
        tx: fmpsc::UnboundedSender<Ev>,
        script: Arc<Script>,
        rate: u32,
        secs: u32,
        cancel: Arc<AtomicBool>,
    ) {
        let provider = MockProvider { script, rate, secs, cancel: cancel.clone() };
        let req = TurnRequest {
            model: "mock-1".into(),
            system: None,
            messages: vec![],
            tools: vec![],
            allow_tool_use: false,
            max_tokens: 4096,
            effort: None,
            thinking: false,
            provider_session: None,
            interaction: None,
            cancel: None,
        };
        let _ = tx.unbounded_send(Ev { at: Instant::now(), kind: EvKind::Start });
        let tx2 = tx.clone();
        let mut count = 0usize;
        let res = provider
            .stream_turn(&req, &mut |ev| {
                let kind = match ev {
                    StreamEvent::Text(t) => {
                        count += 1;
                        EvKind::Text(t.to_string())
                    }
                    StreamEvent::Thinking(t) => EvKind::Thinking(t.to_string()),
                    StreamEvent::ToolCallStart { name, id } => {
                        EvKind::ToolStart { id: id.to_string(), name: name.to_string() }
                    }
                    StreamEvent::ToolCallResult { id, summary, is_error, diff, metadata, .. } => EvKind::ToolResult {
                        artifact: crate::artifact::metadata_document(metadata.as_ref()),
                        id: id.to_string(),
                        summary: summary.to_string(),
                        diff: diff.map(str::to_string),
                        is_error,
                    },
                    _ => return,
                };
                let _ = tx2.unbounded_send(Ev { at: Instant::now(), kind });
            })
            .await;
        let _ = res;
        let _ = tx.unbounded_send(Ev {
            at: Instant::now(),
            kind: EvKind::End { tokens: count, cancelled: cancel.load(Ordering::Relaxed) },
        });
    }
}

#[cfg(not(feature = "core"))]
mod mock {
    use super::*;
    pub async fn turn(
        tx: fmpsc::UnboundedSender<Ev>,
        script: Arc<Script>,
        rate: u32,
        secs: u32,
        cancel: Arc<AtomicBool>,
    ) {
        let _ = tx.unbounded_send(Ev { at: Instant::now(), kind: EvKind::Start });
        let tx2 = tx.clone();
        let mut count = 0usize;
        let (_, cancelled) = pace(&script, rate, secs, &cancel, &mut |p| {
            let kind = match p {
                Piece::Text(t) => {
                    count += 1;
                    EvKind::Text(t.to_string())
                }
                Piece::Think(t) => EvKind::Thinking(t.to_string()),
                Piece::ToolStart { name, id } => EvKind::ToolStart { id: id.to_string(), name: name.to_string() },
                Piece::ToolResult { id, summary, diff, is_error, .. } => EvKind::ToolResult {
                    artifact: None,
                    id: id.to_string(),
                    summary: summary.to_string(),
                    diff: diff.map(str::to_string),
                    is_error,
                },
            };
            let _ = tx2.unbounded_send(Ev { at: Instant::now(), kind });
        })
        .await;
        let _ = tx.unbounded_send(Ev { at: Instant::now(), kind: EvKind::End { tokens: count, cancelled } });
    }
}

pub fn start() -> Bridge {
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<Cmd>();
    let (ev_tx, ev_rx) = fmpsc::unbounded::<Ev>();
    std::thread::Builder::new()
        .name("zest-tokio".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async move {
                let mut cancel = Arc::new(AtomicBool::new(false));
                while let Some(cmd) = cmd_rx.recv().await {
                    match cmd {
                        Cmd::Run(_) | Cmd::Compact => {}
                        Cmd::Cancel => cancel.store(true, Ordering::Relaxed),
                        Cmd::Reply { prompt, images, agent } => {
                            cancel = Arc::new(AtomicBool::new(false));
                            let text = format!(
                                "## Re: {}\n\nYou sent {} chars and {} image(s). This is a **mock** reply streamed from Tokio.\n\n{}\n",
                                prompt.lines().next().unwrap_or("").chars().take(40).collect::<String>(),
                                prompt.chars().count(),
                                images,
                                crate::seed::assistant_text(prompt.len())
                            );
                            let script = Arc::new(if agent {
                                Script::agent(&text, 2, prompt.len())
                            } else {
                                Script::plain(crate::seed::tokenize(&text))
                            });
                            tokio::spawn(mock::turn(ev_tx.clone(), script, 80, 30, cancel.clone()));
                        }
                        Cmd::Stream { rate, secs, chars } => {
                            cancel = Arc::new(AtomicBool::new(false));
                            let script =
                                Arc::new(Script::plain(crate::seed::tokenize(&crate::seed::stream_script(chars))));
                            tokio::spawn(mock::turn(ev_tx.clone(), script, rate, secs, cancel.clone()));
                        }
                        Cmd::Agent { rate, secs, chars, tools } => {
                            cancel = Arc::new(AtomicBool::new(false));
                            let script = Arc::new(Script::agent(&crate::seed::stream_script(chars), tools, chars));
                            tokio::spawn(mock::turn(ev_tx.clone(), script, rate, secs, cancel.clone()));
                        }
                    }
                }
            });
        })
        .expect("spawn tokio thread");
    Bridge { tx: cmd_tx, rx: Some(ev_rx) }
}
