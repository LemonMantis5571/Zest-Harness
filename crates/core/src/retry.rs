//! When and how long a provider request waits before trying again.
//!
//! Shared by every HTTP provider so a hiccup is handled the same way whichever
//! backend the user picked. Modeled on opencode's retry: exponential backoff
//! from two seconds with capped gaps, a server's own `retry-after` honored, and
//! a longer budget when the network itself is unreachable, since that is the
//! failure most likely to clear on its own (a laptop rejoining Wi-Fi).
//!
//! A retry is only safe while nothing has reached the user. Providers ask
//! [`RetryPolicy::delay_before_retry`] only for a request that produced no
//! output; once output has streamed they keep it and let the agent continue.

use std::time::Duration;

use crate::cancel::{wait_cancel, CancelToken};
use crate::error::{HarnessError, Result};
use crate::provider::StreamEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total tries, the first included, for a transient provider failure.
    pub max_attempts: u32,
    /// Total tries when the request never reached a server.
    pub unreachable_max_attempts: u32,
    /// Gap before the first retry; each later gap doubles.
    pub initial_delay: Duration,
    /// Ceiling for a computed gap. A server's `retry-after` is not capped by it.
    pub max_delay: Duration,
    /// A `retry-after` longer than this is not waited out: the error goes to the
    /// user, whose provider message usually says when to come back.
    pub max_retry_after: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 10,
            unreachable_max_attempts: 15,
            initial_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(30),
            max_retry_after: Duration::from_secs(10 * 60),
        }
    }
}

impl RetryPolicy {
    /// The same budget with no waiting, for tests.
    pub fn immediate() -> Self {
        Self {
            initial_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            ..Self::default()
        }
    }

    /// How many tries `error` gets in total.
    pub fn attempts_for(&self, error: &HarnessError) -> u32 {
        if error.is_unreachable() {
            self.unreachable_max_attempts
        } else {
            self.max_attempts
        }
    }

    /// The wait before try `attempt + 1`, or `None` to give up.
    ///
    /// `attempt` counts tries already made, starting at 1. `error` must be a
    /// failure that produced no output; whether it is worth retrying at all is
    /// decided here too.
    pub fn delay_before_retry(
        &self,
        attempt: u32,
        error: &HarnessError,
        retry_after: Option<Duration>,
    ) -> Option<Duration> {
        if !is_retryable(error) || attempt >= self.attempts_for(error) {
            return None;
        }
        if let Some(wait) = retry_after {
            return (wait <= self.max_retry_after).then_some(wait);
        }
        let doubled = self
            .initial_delay
            .saturating_mul(1u32 << attempt.saturating_sub(1).min(16));
        Some(doubled.min(self.max_delay) + jitter(self.max_delay))
    }
}

/// Whether a failure that produced no output is worth another try.
pub fn is_retryable(error: &HarnessError) -> bool {
    error.is_transient() || error.is_retryable_stream_failure()
}

/// Up to a quarter second, so concurrent workers do not retry in lockstep.
/// Derived from the clock rather than a PRNG to avoid a dependency.
fn jitter(max_delay: Duration) -> Duration {
    if max_delay.is_zero() {
        return Duration::ZERO;
    }
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos() % 250_000_000) / 1_000_000)
        .unwrap_or(0);
    Duration::from_millis(millis)
}

/// Show a retry as a provider activity row, wait it out, and mark the row
/// done. Returns `Cancelled` at once if the user stops the turn while waiting.
pub async fn wait_for_retry(
    delay: Duration,
    attempt: u32,
    max_attempts: u32,
    error: &HarnessError,
    on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    cancel: Option<&CancelToken>,
) -> Result<()> {
    let id = crate::thread::new_id("retry");
    let title = format!(
        "{}. Retrying in {}s (attempt {} of {max_attempts})",
        retry_reason(error),
        delay.as_secs().max(1),
        attempt + 1,
    );
    on_event(StreamEvent::ProviderActivity {
        id: &id,
        title: &title,
        status: "running",
    });
    let cancelled = tokio::select! {
        biased;
        _ = wait_cancel(cancel) => true,
        _ = tokio::time::sleep(delay) => false,
    };
    on_event(StreamEvent::ProviderActivity {
        id: &id,
        title: &title,
        status: if cancelled { "error" } else { "completed" },
    });
    if cancelled {
        Err(HarnessError::Cancelled)
    } else {
        Ok(())
    }
}

/// A short, credential-free account of why a request is being retried.
pub fn retry_reason(error: &HarnessError) -> String {
    match error.root() {
        HarnessError::Api { status: 429, .. } => "The provider is rate limiting requests".into(),
        HarnessError::Api {
            status: 503 | 529, ..
        } => "The provider is overloaded".into(),
        HarnessError::Api { status, .. } => format!("The provider returned an error ({status})"),
        HarnessError::ResponseTimeout { .. } => "The provider did not answer in time".into(),
        HarnessError::Stream { kind, .. } if kind.contains("overloaded") => {
            "The provider is overloaded".into()
        }
        HarnessError::Stream { kind, .. } if kind.contains("rate_limit") => {
            "The provider is rate limiting requests".into()
        }
        error if error.is_unreachable() => "Zest could not reach the provider".into(),
        _ => "The connection to the provider dropped".into(),
    }
}

/// A failed try: the error and any `retry-after` the server asked for.
pub type AttemptError = (HarnessError, Option<Duration>);

/// One provider request and its stream, tried again by [`run_with_retry`].
#[async_trait::async_trait]
pub trait Attempt: Send {
    /// Make one try. Fail only when nothing reached the user: a try that
    /// streamed visible text before breaking off returns that text, stopped
    /// with [`crate::provider::STREAM_INTERRUPTED`].
    async fn run(
        &mut self,
        on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    ) -> std::result::Result<crate::provider::Completion, AttemptError>;

    /// A provider's own veto on retrying an otherwise retryable failure.
    fn may_retry(&self, _error: &HarnessError) -> bool {
        true
    }
}

/// Run `attempt` until it succeeds, fails for good, or the budget runs out.
/// Each wait shows as a provider activity row and ends at once on cancel.
pub async fn run_with_retry(
    policy: &RetryPolicy,
    attempt: &mut dyn Attempt,
    on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    cancel: Option<&CancelToken>,
) -> Result<crate::provider::Completion> {
    let mut tries = 0u32;
    loop {
        tries += 1;
        let (error, retry_after) = match attempt.run(on_event).await {
            Ok(completion) => return Ok(completion),
            Err(failure) => failure,
        };
        if matches!(error, HarnessError::Cancelled) {
            return Err(error);
        }
        let delay = attempt
            .may_retry(&error)
            .then(|| policy.delay_before_retry(tries, &error, retry_after))
            .flatten();
        let Some(delay) = delay else {
            return Err(exhausted(error, tries));
        };
        let max_attempts = policy.attempts_for(&error);
        wait_for_retry(delay, tries, max_attempts, &error, on_event, cancel).await?;
    }
}

/// Wrap the last failure with how many tries were spent, unless it is a
/// cancellation (the caller branches on that variant) or there was one try.
pub fn exhausted(error: HarnessError, attempts: u32) -> HarnessError {
    match error {
        HarnessError::Cancelled => HarnessError::Cancelled,
        other if attempts > 1 => HarnessError::Exhausted {
            attempts,
            source: Box::new(other),
        },
        other => other,
    }
}

/// `retry-after-ms` or `retry-after` (seconds or an HTTP date).
pub fn retry_after_from_headers(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let text = |name: &str| headers.get(name)?.to_str().ok().map(str::trim);
    if let Some(millis) = text("retry-after-ms").and_then(|v| v.parse::<f64>().ok()) {
        if millis.is_finite() && millis >= 0.0 {
            return Some(Duration::from_millis(millis as u64));
        }
    }
    let value = text("retry-after")?;
    if let Ok(seconds) = value.parse::<f64>() {
        return (seconds.is_finite() && seconds >= 0.0)
            .then(|| Duration::from_millis((seconds * 1000.0) as u64));
    }
    let at = httpdate::parse_http_date(value).ok()?;
    Some(
        at.duration_since(std::time::SystemTime::now())
            .unwrap_or(Duration::ZERO),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Completion;

    fn overloaded() -> HarnessError {
        HarnessError::Api {
            status: 529,
            body: String::new(),
        }
    }

    #[test]
    fn backoff_doubles_from_two_seconds_and_caps_at_thirty() {
        let policy = RetryPolicy::default();
        let gaps: Vec<u64> = (1..policy.max_attempts)
            .map(|attempt| {
                policy
                    .delay_before_retry(attempt, &overloaded(), None)
                    .unwrap()
                    .as_secs()
            })
            .collect();
        assert_eq!(gaps, [2, 4, 8, 16, 30, 30, 30, 30, 30]);
        assert_eq!(
            policy.delay_before_retry(policy.max_attempts, &overloaded(), None),
            None
        );
    }

    #[test]
    fn a_server_retry_after_is_honored_until_it_is_absurd() {
        let policy = RetryPolicy::default();
        assert_eq!(
            policy.delay_before_retry(1, &overloaded(), Some(Duration::from_secs(90))),
            Some(Duration::from_secs(90))
        );
        assert_eq!(
            policy.delay_before_retry(1, &overloaded(), Some(Duration::from_secs(3600))),
            None
        );
    }

    #[test]
    fn stream_drops_retry_but_bad_requests_do_not() {
        let policy = RetryPolicy::immediate();
        for error in [
            HarnessError::PrematureEof,
            HarnessError::StreamIdleTimeout,
            HarnessError::from_provider_stream("overloaded_error", "Overloaded"),
        ] {
            assert!(
                policy.delay_before_retry(1, &error, None).is_some(),
                "{error}"
            );
        }
        for error in [
            HarnessError::Api {
                status: 400,
                body: String::new(),
            },
            HarnessError::from_provider_stream("usage_limit_reached", "Try again tomorrow"),
            HarnessError::Cancelled,
        ] {
            assert!(
                policy.delay_before_retry(1, &error, None).is_none(),
                "{error}"
            );
        }
    }

    #[tokio::test]
    async fn an_unreachable_provider_gets_the_longer_budget() {
        let addr = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let error = HarnessError::Http(
            reqwest::Client::new()
                .get(format!("http://{addr}"))
                .send()
                .await
                .unwrap_err(),
        );
        let policy = RetryPolicy::default();
        assert!(error.is_unreachable());
        assert_eq!(policy.attempts_for(&error), policy.unreachable_max_attempts);
        assert!(policy.unreachable_max_attempts > policy.max_attempts);
        assert!(policy
            .delay_before_retry(policy.max_attempts, &error, None)
            .is_some());
    }

    /// Plays back one outcome per try.
    struct Scripted(std::collections::VecDeque<std::result::Result<Completion, AttemptError>>);

    #[async_trait::async_trait]
    impl Attempt for Scripted {
        async fn run(
            &mut self,
            _on_event: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
        ) -> std::result::Result<Completion, AttemptError> {
            self.0.pop_front().expect("no more scripted tries")
        }
    }

    fn done() -> Completion {
        Completion {
            content: Vec::new(),
            stop_reason: Some("end_turn".into()),
            usage: Default::default(),
            usage_available: true,
            limits: None,
            served_model: None,
            provider_session: None,
        }
    }

    #[tokio::test]
    async fn failures_before_output_are_retried_and_shown() {
        let mut attempt = Scripted(
            vec![
                Err((overloaded(), None)),
                Err((HarnessError::PrematureEof, None)),
                Ok(done()),
            ]
            .into(),
        );
        let mut statuses = Vec::new();
        let mut sink = |event: StreamEvent<'_>| {
            if let StreamEvent::ProviderActivity { status, .. } = event {
                statuses.push(status.to_string());
            }
        };
        run_with_retry(&RetryPolicy::immediate(), &mut attempt, &mut sink, None)
            .await
            .unwrap();
        assert!(attempt.0.is_empty());
        assert_eq!(statuses, ["running", "completed", "running", "completed"]);
    }

    #[tokio::test]
    async fn a_permanent_failure_is_returned_at_once() {
        let mut attempt = Scripted(
            vec![Err((
                HarnessError::Api {
                    status: 400,
                    body: "bad".into(),
                },
                None,
            ))]
            .into(),
        );
        let error = run_with_retry(&RetryPolicy::immediate(), &mut attempt, &mut |_| {}, None)
            .await
            .unwrap_err();
        assert!(matches!(error, HarnessError::Api { status: 400, .. }));
    }

    #[tokio::test]
    async fn stop_ends_a_retry_wait_at_once() {
        let mut attempt = Scripted(vec![Err((overloaded(), None))].into());
        let cancel = CancelToken::new();
        let token = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            token.cancel();
        });
        let started = std::time::Instant::now();
        let error = run_with_retry(
            &RetryPolicy::default(),
            &mut attempt,
            &mut |_| {},
            Some(&cancel),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, HarnessError::Cancelled));
        assert!(started.elapsed() < Duration::from_millis(1500));
    }

    #[test]
    fn retry_after_reads_milliseconds_seconds_and_dates() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "5".parse().unwrap());
        assert_eq!(
            retry_after_from_headers(&headers),
            Some(Duration::from_secs(5))
        );
        headers.insert("retry-after-ms", "1500".parse().unwrap());
        assert_eq!(
            retry_after_from_headers(&headers),
            Some(Duration::from_millis(1500))
        );
        let mut headers = reqwest::header::HeaderMap::new();
        let later = std::time::SystemTime::now() + Duration::from_secs(120);
        headers.insert(
            "retry-after",
            httpdate::fmt_http_date(later).parse().unwrap(),
        );
        let wait = retry_after_from_headers(&headers).unwrap();
        assert!(wait > Duration::from_secs(100) && wait <= Duration::from_secs(120));
    }
}
