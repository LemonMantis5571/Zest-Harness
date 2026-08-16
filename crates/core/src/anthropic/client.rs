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
use super::types::{Request, API_BASE, API_VERSION};
use crate::cancel::{wait_cancel, CancelToken};
use crate::error::{HarnessError, Result};
use crate::provider::{Completion, RateLimitSnapshot, StreamEvent};

/// TCP/TLS connect budget.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Max silence between SSE chunks before the turn fails.
pub const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// Tries for a request that has produced no output yet. Small on purpose: a
/// coding turn is long, and the user is watching.
pub const MAX_ATTEMPTS: u32 = 3;

pub struct AnthropicClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl AnthropicClient {
    pub fn new(api_key: String) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .build()?,
            api_key,
            base_url: API_BASE.to_string(),
        })
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
        // Retry only covers getting a successful response. Once the body starts
        // streaming a second attempt would replay text the caller already saw,
        // so everything below the loop happens exactly once.
        let mut attempt = 0u32;
        let resp = loop {
            attempt += 1;
            let (error, retry_after) = match self.send_once(req, cancel).await {
                Ok(resp) => break resp,
                Err(pair) => pair,
            };

            let exhausted = attempt >= MAX_ATTEMPTS;
            if exhausted || !error.is_transient() {
                return Err(if attempt > 1 {
                    annotate_attempts(error, attempt)
                } else {
                    error
                });
            }

            let delay = retry_after.unwrap_or_else(|| backoff(attempt));
            tokio::select! {
                biased;
                _ = wait_cancel(cancel) => return Err(HarnessError::Cancelled),
                _ = tokio::time::sleep(delay) => {}
            }
        };

        // Read the headers before touching the body — `bytes_stream()` consumes
        // the response.
        let limits = rate_limits_from_headers(resp.headers());

        let mut accumulator = TurnAccumulator::new();
        let mut parser = SseParser::default();
        let mut body = resp.bytes_stream();

        loop {
            tokio::select! {
                biased;

                _ = wait_cancel(cancel) => {
                    // Dropping `body` aborts the HTTP connection.
                    return Err(HarnessError::Cancelled);
                }

                chunk = body.next() => {
                    match chunk {
                        Some(Ok(chunk)) => {
                            for payload in parser.feed(&chunk) {
                                let event: Value = serde_json::from_str(&payload)?;
                                accumulator.push(&event, on_event)?;
                            }
                            if accumulator.is_done() {
                                break;
                            }
                        }
                        Some(Err(e)) => return Err(e.into()),
                        None => break,
                    }
                }

                _ = tokio::time::sleep(STREAM_IDLE_TIMEOUT) => {
                    return Err(HarnessError::StreamIdleTimeout);
                }
            }
        }

        if !accumulator.is_done() {
            return Err(HarnessError::PrematureEof);
        }

        Ok(accumulator.finish(limits))
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

        let retry_after = retry_after_from_headers(resp.headers());
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

/// Exponential backoff with jitter: ~1s, ~2s, ~4s.
///
/// The jitter is derived from the wall clock rather than a PRNG to avoid a
/// dependency for something this small. Its only job is to stop two concurrent
/// delegated workers from retrying in lockstep.
fn backoff(attempt: u32) -> Duration {
    let base = Duration::from_secs(1 << attempt.min(4).saturating_sub(1));
    let jitter_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos() % 250_000_000) / 1_000_000)
        .unwrap_or(0);
    base + Duration::from_millis(jitter_ms)
}

/// `retry-after` as either delay-seconds or an HTTP date. Only the seconds form
/// is honoured; a date would need a date library for a marginal gain.
fn retry_after_from_headers(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let secs: u64 = headers
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    // Do not let a server park the turn for minutes; fall through to our own
    // backoff by reporting nothing.
    (secs <= 60).then(|| Duration::from_secs(secs))
}

/// Make an exhausted retry visible in the message the user actually reads.
///
/// Wraps rather than reformats. Editing the failure into a string used to cost
/// two things: `Http` lost `is_connect()`, so a gateway that was not running
/// reported as a broken session, and the suffix appended to an `Api` body left
/// it no longer parseable as the JSON error envelope it is.
///
/// `Cancelled` is passed through untouched: the caller branches on that variant
/// to distinguish "the user stopped it" from "it broke", and wrapping it would
/// report a deliberate Stop as a failure.
fn annotate_attempts(error: HarnessError, attempts: u32) -> HarnessError {
    match error {
        HarnessError::Cancelled => HarnessError::Cancelled,
        other => HarnessError::Exhausted {
            attempts,
            source: Box::new(other),
        },
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
    use std::sync::Arc;

    use crate::anthropic::types::Message;

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
                            "HTTP/1.1 {status} X\r\n{header}content-length: 4\r\n\r\nboom"
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
                            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n{sse}",
                            sse.len()
                        );
                    }
                }
                let _ = stream.flush();
            }
        });

        (format!("http://{addr}"), hits)
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
        let (base, hits) = spawn_server(vec![529, 529, 529, 529]);
        let client = AnthropicClient::new("k".into())
            .unwrap()
            .with_base_url(base);
        let mut sink = |_ev: StreamEvent<'_>| {};
        let err = client.stream(&request(), &mut sink).await.unwrap_err();
        assert_eq!(hits.load(Ordering::SeqCst), MAX_ATTEMPTS as usize);
        let message = err.to_string();
        assert!(message.contains("failed after 3 attempts"), "{message}");
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
    #[tokio::test]
    async fn a_dead_port_stays_classified_as_unreachable() {
        // Bind then drop, so the port is real but nothing is accepting on it.
        let addr = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let client = AnthropicClient::new("k".into())
            .unwrap()
            .with_base_url(format!("http://{addr}"));
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

    #[test]
    fn retry_after_ignores_absurd_delays() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "5".parse().unwrap());
        assert_eq!(
            retry_after_from_headers(&headers),
            Some(Duration::from_secs(5))
        );
        headers.insert("retry-after", "3600".parse().unwrap());
        assert_eq!(retry_after_from_headers(&headers), None);
        headers.insert(
            "retry-after",
            "Wed, 21 Oct 2026 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(retry_after_from_headers(&headers), None);
    }
}
