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
    /// A user turn: stream a canned reply for `secs` at `rate` tokens/s.
    Reply { prompt: String, images: usize },
    Stream { rate: u32, secs: u32, chars: usize },
    Cancel,
}

pub struct Ev {
    /// Stamped on the Tokio thread, compared on the UI thread: end-to-end wake-up latency.
    pub at: Instant,
    pub kind: EvKind,
}

pub enum EvKind {
    Start,
    Thinking(String),
    Text(String),
    End { tokens: usize, cancelled: bool },
}

pub struct Bridge {
    pub tx: tokio::sync::mpsc::UnboundedSender<Cmd>,
    pub rx: Option<fmpsc::UnboundedReceiver<Ev>>,
}

pub enum Piece<'a> {
    Think(&'a str),
    Text(&'a str),
}

/// Emits `tokens` at roughly `rate` per second, catching up after slow timer ticks.
pub async fn pace(
    tokens: &[String],
    rate: u32,
    max_secs: u32,
    cancel: &AtomicBool,
    emit: &mut (dyn FnMut(Piece) + Send),
) -> (usize, bool) {
    for t in ["Looking ", "at ", "the ", "request, ", "then ", "drafting ", "an ", "answer."] {
        emit(Piece::Think(t));
        tokio::time::sleep(Duration::from_millis(20)).await;
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
        pub tokens: Arc<Vec<String>>,
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
            let (sent, _) = pace(&self.tokens, self.rate, self.secs, &self.cancel, &mut |p| match p {
                Piece::Think(t) => on_event(StreamEvent::Thinking(t)),
                Piece::Text(t) => on_event(StreamEvent::Text(t)),
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
        tokens: Arc<Vec<String>>,
        rate: u32,
        secs: u32,
        cancel: Arc<AtomicBool>,
    ) {
        let provider = MockProvider { tokens, rate, secs, cancel: cancel.clone() };
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
        tokens: Arc<Vec<String>>,
        rate: u32,
        secs: u32,
        cancel: Arc<AtomicBool>,
    ) {
        let _ = tx.unbounded_send(Ev { at: Instant::now(), kind: EvKind::Start });
        let tx2 = tx.clone();
        let mut count = 0usize;
        let (_, cancelled) = pace(&tokens, rate, secs, &cancel, &mut |p| {
            let kind = match p {
                Piece::Text(t) => {
                    count += 1;
                    EvKind::Text(t.to_string())
                }
                Piece::Think(t) => EvKind::Thinking(t.to_string()),
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
                        Cmd::Cancel => cancel.store(true, Ordering::Relaxed),
                        Cmd::Reply { prompt, images } => {
                            cancel = Arc::new(AtomicBool::new(false));
                            let text = format!(
                                "## Re: {}\n\nYou sent {} chars and {} image(s). This is a **mock** reply streamed from Tokio.\n\n{}\n",
                                prompt.lines().next().unwrap_or("").chars().take(40).collect::<String>(),
                                prompt.chars().count(),
                                images,
                                crate::seed::assistant_text(prompt.len())
                            );
                            let tokens = Arc::new(crate::seed::tokenize(&text));
                            tokio::spawn(mock::turn(ev_tx.clone(), tokens, 80, 30, cancel.clone()));
                        }
                        Cmd::Stream { rate, secs, chars } => {
                            cancel = Arc::new(AtomicBool::new(false));
                            let tokens = Arc::new(crate::seed::tokenize(&crate::seed::stream_script(chars)));
                            tokio::spawn(mock::turn(ev_tx.clone(), tokens, rate, secs, cancel.clone()));
                        }
                    }
                }
            });
        })
        .expect("spawn tokio thread");
    Bridge { tx: cmd_tx, rx: Some(ev_rx) }
}
