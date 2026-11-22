//! Streaming Messages API client.
//!
//! Owns the HTTP request and the SSE transport. Rebuilding the assistant turn
//! from the event stream lives in `accumulate.rs` so it can be tested against a
//! recorded transcript rather than only over the network.

use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;

use super::accumulate::TurnAccumulator;
use super::sse::SseParser;
use super::types::{Request, API_BASE, API_VERSION, THINKING_BINDING_BETA};
use crate::cancel::{wait_cancel, CancelToken};
use crate::error::{HarnessError, Result};
use crate::provider::{Completion, RateLimitSnapshot, StreamEvent};
use crate::retry::{self, RetryPolicy};

/// TCP/TLS connect budget.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Max silence between SSE chunks before the turn fails.
pub const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
pub struct AnthropicClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    retry: RetryPolicy,
}

impl AnthropicClient {
    pub fn new(api_key: String) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .build()?,
            api_key,
            base_url: API_BASE.to_string(),
            retry: RetryPolicy::default(),
        })
    }

    /// Replace the retry budget, e.g. with [`RetryPolicy::immediate`] in tests.
    pub fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Point at something other than the Anthropic API — a gateway that speaks
    /// the Messages API on behalf of another backend, or a local mock.
    ///
    /// Takes an origin, not a full endpoint: `http://127.0.0.1:8317`, not
    /// `.../v1/messages`.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }

    pub async fn stream(
        &self,
        req: &Request,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    ) -> Result<Completion> {
        self.stream_cancellable(req, on_event, None).await
    }

    pub async fn stream_cancellable(
        &self,
        req: &Request,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
        cancel: Option<&CancelToken>,
    ) -> Result<Completion> {
        // A try that failed before anything reached the user is retried; one
        // that already streamed text returns that text instead (see
        // `try_once`), so a retry never replays output the caller has seen.
        let mut attempt = AnthropicAttempt {
            client: self,
            req,
            cancel,
        };
        retry::run_with_retry(&self.retry, &mut attempt, on_event, cancel).await
    }

    /// One request and its stream.
    ///
    /// A stream that drops, stalls or reports an overload after visible text
    /// has streamed returns that text as a completion stopped with
    /// [`crate::provider::STREAM_INTERRUPTED`], and the agent asks the model to continue. With
    /// nothing visible yet it is an error the caller may retry.
    async fn try_once(
        &self,
        req: &Request,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
        cancel: Option<&CancelToken>,
    ) -> std::result::Result<Completion, retry::AttemptError> {
        let resp = self.send_once(req, cancel).await?;

        // Read the headers before touching the body — `bytes_stream()` consumes
        // the response.
        let limits = rate_limits_from_headers(resp.headers());

        let mut accumulator = TurnAccumulator::new();
        let mut parser = SseParser::default();
        let mut body = resp.bytes_stream();

        let failure = 'read: loop {
            tokio::select! {
                biased;

                _ = wait_cancel(cancel) => {
                    // Dropping `body` aborts the HTTP connection.
                    return Err((HarnessError::Cancelled, None));
                }

                chunk = body.next() => {
                    match chunk {
                        Some(Ok(chunk)) => {
                            for payload in parser.feed(&chunk) {
                                let pushed = serde_json::from_str::<Value>(&payload)
                                    .map_err(HarnessError::from)
                                    .and_then(|event| accumulator.push(&event, on_event));
                                if let Err(error) = pushed {
                                    break 'read Some(error);
                                }
                            }
                            if accumulator.is_done() {
                                break None;
                            }
                        }
                        Some(Err(e)) => break Some(e.into()),
                        None => break None,
                    }
                }

                _ = tokio::time::sleep(STREAM_IDLE_TIMEOUT) => {
                    break Some(HarnessError::StreamIdleTimeout);
                }
            }
        };

        let failure =
            failure.or_else(|| (!accumulator.is_done()).then_some(HarnessError::PrematureEof));
        let Some(error) = failure else {
            return Ok(accumulator.finish(limits));
        };
        if error.is_retryable_stream_failure() {
            if let Some(partial) = accumulator.salvage(limits) {
                return Ok(partial);
            }
        }
        Err((error, None))
    }

    /// One attempt at getting a streaming response.
    ///
    /// On failure returns the error alongside any `retry-after` the server
    /// asked for — a rate limiter's own number is better than our guess.
    async fn send_once(
        &self,
        req: &Request,
        cancel: Option<&CancelToken>,
    ) -> std::result::Result<reqwest::Response, (HarnessError, Option<Duration>)> {
        let resp = tokio::select! {
            biased;
            _ = wait_cancel(cancel) => return Err((HarnessError::Cancelled, None)),
            resp = self
                .http
                .post(self.endpoint())
                .header("x-api-key", &self.api_key)
                // Gateways commonly read the bearer header instead. Sending both is
                // harmless — the real API ignores it.
                .header("authorization", format!("Bearer {}", self.api_key))
                .header("anthropic-version", API_VERSION)
                .header("anthropic-beta", THINKING_BINDING_BETA)
                .header("content-type", "application/json")
                .json(req)
                .send() => match resp {
                    Ok(resp) => resp,
                    Err(e) => return Err((HarnessError::Http(e), None)),
                },
        };

        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }

        let retry_after = retry::retry_after_from_headers(resp.headers());
        let body = resp.text().await.unwrap_or_default();
        Err((
            HarnessError::Api {
                status: status.as_u16(),
                body,
            },
            retry_after,
        ))
    }
}

struct AnthropicAttempt<'c> {
    client: &'c AnthropicClient,
    req: &'c Request,
    cancel: Option<&'c CancelToken>,
}

#[async_trait::async_trait]
impl retry::Attempt for AnthropicAttempt<'_> {
    async fn run(
        &mut self,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    ) -> std::result::Result<Completion, retry::AttemptError> {
        self.client.try_once(self.req, on_event, self.cancel).await
    }
}

/// Throughput headroom, if the endpoint reports it.
///
/// Anthropic sends these; a gateway generally does not, and `None` is the honest
/// answer there rather than a fabricated zero.
fn rate_limits_from_headers(headers: &reqwest::header::HeaderMap) -> Option<RateLimitSnapshot> {
    let text = |key: &str| {
        headers
            .get(key)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let number = |key: &str| text(key).and_then(|v| v.parse::<u64>().ok());

    let snapshot = RateLimitSnapshot {
        requests_limit: number("anthropic-ratelimit-requests-limit"),
        requests_remaining: number("anthropic-ratelimit-requests-remaining"),
        requests_reset: text("anthropic-ratelimit-requests-reset"),
        tokens_limit: None,
        tokens_remaining: None,
        input_tokens_remaining: number("anthropic-ratelimit-input-tokens-remaining"),
        output_tokens_remaining: number("anthropic-ratelimit-output-tokens-remaining"),
        tokens_reset: text("anthropic-ratelimit-tokens-reset"),
        retry_after_secs: number("retry-after"),
        ..Default::default()
    };

    (!snapshot.is_empty()).then_some(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, OnceLock};

    use crate::anthropic::types::Message;

    async fn network_test_guard() -> tokio::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await
    }

    /// Fast tests pin the backoff to zero via the server's own `retry-after`.
    fn spawn_server(statuses: Vec<u16>) -> (String, Arc<AtomicUsize>) {
        spawn_server_with(statuses, Some(0))
    }

    /// Minimal one-shot HTTP server: replies with `statuses[n]` to the nth
    /// request, then a canned SSE turn once the status list runs out.
    ///
    /// `retry_after` of `None` omits the header, so the client falls back to its
    /// own second-scale backoff — which is what the cancel test needs to have
    /// something to interrupt.
    fn spawn_server_with(
        statuses: Vec<u16>,
        retry_after: Option<u64>,
    ) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();

        // Each response closes its connection and says so. Without the header
        // the client pools the socket and sends the retry down one the server
        // has already dropped, which Windows reports as a connection reset.
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let n = counter.fetch_add(1, Ordering::SeqCst);

                // Drain the request head so the client is not left blocked.
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut content_length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                    if line == "\r\n" || line == "\n" {
                        break;
                    }
                }
                if content_length > 0 {
                    use std::io::Read;
                    let mut body = vec![0u8; content_length];
                    let _ = reader.read_exact(&mut body);
                }

                match statuses.get(n) {
                    Some(&status) => {
                        let header = match retry_after {
                            Some(secs) => format!("retry-after: {secs}\r\n"),
                            None => String::new(),
                        };
                        let _ = write!(
                            stream,
                            "HTTP/1.1 {status} X\r\n{header}content-length: 4\r\nconnection: close\r\n\r\nboom"
                        );
                    }
                    None => {
                        let sse = concat!(
                            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
                            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n",
                            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
                            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
                        );
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
                            sse.len()
                        );
                    }
                }
                let _ = stream.flush();
            }
        });

        (format!("http://{addr}"), hits)
    }

    /// Replies to the nth request with `responses[n]` verbatim and closes the
    /// connection, so a response can stop part-way through its stream.
    fn spawn_raw_server(responses: Vec<String>) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let n = counter.fetch_add(1, Ordering::SeqCst);
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut content_length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; content_length];
                let _ = std::io::Read::read_exact(&mut reader, &mut body);
                let Some(response) = responses.get(n) else {
                    break;
                };
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        (format!("http://{addr}"), hits)
    }

    /// An SSE body cut off after `events`, with no `message_stop`.
    fn truncated_stream(events: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{events}",
            events.len()
        )
    }

    const START: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n";
    const TEXT_BLOCK: &str = concat!(
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Half an answer\"}}\n\n",
    );
    const FINISH: &str = concat!(
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    );

    #[tokio::test]
    async fn a_stream_that_drops_before_any_text_is_retried() {
        let _guard = network_test_guard().await;
        let overloaded = format!(
            "{START}event: error\ndata: {{\"type\":\"error\",\"error\":{{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}}}\n\n"
        );
        let (base, hits) = spawn_raw_server(vec![
            truncated_stream(START),
            truncated_stream(&overloaded),
            truncated_stream(&format!("{START}{TEXT_BLOCK}{FINISH}")),
        ]);
        let client = AnthropicClient::new("k".into())
            .unwrap()
            .with_base_url(base)
            .with_retry_policy(RetryPolicy::immediate());
        let mut retries = 0;
        let mut sink = |event: StreamEvent<'_>| {
            if matches!(
                event,
                StreamEvent::ProviderActivity {
                    status: "running",
                    ..
                }
            ) {
                retries += 1;
            }
        };
        let completion = client.stream(&request(), &mut sink).await.unwrap();
        assert_eq!(completion.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        assert_eq!(retries, 2);
    }

    #[tokio::test]
    async fn a_stream_that_drops_after_text_keeps_the_text() {
        let _guard = network_test_guard().await;
        let (base, hits) =
            spawn_raw_server(vec![truncated_stream(&format!("{START}{TEXT_BLOCK}"))]);
        let client = AnthropicClient::new("k".into())
            .unwrap()
            .with_base_url(base)
            .with_retry_policy(RetryPolicy::immediate());
        let mut seen = String::new();
        let mut sink = |event: StreamEvent<'_>| {
            if let StreamEvent::Text(text) = event {
                seen.push_str(text);
            }
        };
        let completion = client.stream(&request(), &mut sink).await.unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 1, "must not replay seen text");
        assert_eq!(seen, "Half an answer");
        assert_eq!(
            completion.stop_reason.as_deref(),
            Some(crate::provider::STREAM_INTERRUPTED)
        );
        assert_eq!(completion.content[0]["text"], "Half an answer");
    }

    fn request() -> Request {
        Request {
            model: "test".into(),
            max_tokens: 16,
            stream: true,
            system: None,
            messages: vec![Message::user_text("hi")],
            tools: Vec::new(),
            tool_choice: None,
            thinking: None,
            output_config: None,
        }
    }

    #[tokio::test]
    async fn retries_a_529_then_succeeds() {
        let _guard = network_test_guard().await;
        let (base, hits) = spawn_server(vec![529]);
        let client = AnthropicClient::new("k".into())
            .unwrap()
            .with_base_url(base);
        let mut sink = |_ev: StreamEvent<'_>| {};
        let completion = client.stream(&request(), &mut sink).await.unwrap();
        assert_eq!(completion.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(hits.load(Ordering::SeqCst), 2, "should have retried once");
    }

    #[tokio::test]
    async fn gives_up_after_max_attempts_and_says_so() {
        let _guard = network_test_guard().await;
        let policy = RetryPolicy::immediate();
        let (base, hits) = spawn_server(vec![529; policy.max_attempts as usize + 1]);
        let client = AnthropicClient::new("k".into())
            .unwrap()
            .with_base_url(base)
            .with_retry_policy(policy.clone());
        let mut sink = |_ev: StreamEvent<'_>| {};
        let err = client.stream(&request(), &mut sink).await.unwrap_err();
        assert_eq!(hits.load(Ordering::SeqCst), policy.max_attempts as usize);
        let message = err.to_string();
        assert!(
            message.contains(&format!("failed after {} attempts", policy.max_attempts)),
            "{message}"
        );
        // Giving up must not rewrite the body: it is the API's error envelope,
        // and the desktop parses it to show the provider's own wording.
        assert!(
            matches!(err.root(), HarnessError::Api { status: 529, body } if body == "boom"),
            "{err:?}"
        );
    }

    /// A refused connection is the most common alpha failure — the local gateway
    /// is simply not running. It is retried, so it always reaches the annotation
    /// path, and it must still be recognisable as a transport failure afterwards.
    /// Formatting it into a string was reporting it as a bad Claude session.
    /// Without the beta header the API rejects `thinking.block_binding` as an
    /// unknown field, so the pair has to travel together on every request.
    #[tokio::test]
    async fn thinking_requests_send_block_binding_with_its_beta_header() {
        let _guard = network_test_guard().await;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let captured = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = Vec::new();
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                let lower = line.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
                head.push(lower.trim().to_string());
            }
            let mut body = vec![0u8; content_length];
            std::io::Read::read_exact(&mut reader, &mut body).unwrap();
            let sse = concat!(
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
                "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
                "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            );
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
                sse.len()
            );
            (head, String::from_utf8(body).unwrap())
        });

        let client = AnthropicClient::new("k".into())
            .unwrap()
            .with_base_url(format!("http://{addr}"));
        let mut thinking = request();
        thinking.thinking = Some(super::super::types::Thinking::default());
        let mut sink = |_ev: StreamEvent<'_>| {};
        client.stream(&thinking, &mut sink).await.unwrap();

        let (head, body) = captured.join().unwrap();
        assert!(
            head.iter()
                .any(|line| line == &format!("anthropic-beta: {THINKING_BINDING_BETA}")),
            "{head:?}"
        );
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(
            body["thinking"]["block_binding"]["prefix_mismatch_behavior"],
            "drop_block"
        );
    }

    #[tokio::test]
    async fn a_dead_port_stays_classified_as_unreachable() {
        let _guard = network_test_guard().await;
        // Bind then drop, so the port is real but nothing is accepting on it.
        let addr = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let client = AnthropicClient::new("k".into())
            .unwrap()
            .with_base_url(format!("http://{addr}"))
            .with_retry_policy(RetryPolicy::immediate());
        let mut sink = |_ev: StreamEvent<'_>| {};
        let err = client.stream(&request(), &mut sink).await.unwrap_err();

        assert!(err.is_unreachable(), "{err:?}");
        assert!(
            !err.is_auth_problem(),
            "must not send the user through a sign-in: {err}"
        );
        assert!(err.to_string().contains("failed after"), "{err}");
    }

    #[tokio::test]
    async fn does_not_retry_a_bad_request() {
        let _guard = network_test_guard().await;
        // A 400 is deterministic — retrying it just spends time and quota.
        let (base, hits) = spawn_server(vec![400, 400]);
        let client = AnthropicClient::new("k".into())
            .unwrap()
            .with_base_url(base);
        let mut sink = |_ev: StreamEvent<'_>| {};
        let err = client.stream(&request(), &mut sink).await.unwrap_err();
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert!(matches!(err, HarnessError::Api { status: 400, .. }));
    }

    /// Stop must not have to wait out a backoff sleep.
    #[tokio::test]
    async fn cancel_during_backoff_returns_immediately() {
        let _guard = network_test_guard().await;
        // No `retry-after`, so the client uses its own ~1s backoff — there is
        // an actual sleep to interrupt.
        let (base, hits) = spawn_server_with(vec![503, 503, 503, 503], None);
        let client = AnthropicClient::new("k".into())
            .unwrap()
            .with_base_url(base);
        let cancel = CancelToken::new();
        let token = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            token.cancel();
        });

        let mut sink = |_ev: StreamEvent<'_>| {};
        let started = std::time::Instant::now();
        let err = client
            .stream_cancellable(&request(), &mut sink, Some(&cancel))
            .await
            .unwrap_err();

        assert!(matches!(err, HarnessError::Cancelled), "{err}");
        assert!(
            started.elapsed() < Duration::from_millis(800),
            "cancel waited out the backoff: {:?}",
            started.elapsed()
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1, "must not keep retrying");
    }
}
