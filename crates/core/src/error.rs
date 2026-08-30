use thiserror::Error;

#[derive(Debug, Error)]
pub enum HarnessError {
    #[error("http transport: {0}")]
    Http(#[from] reqwest::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    /// Non-2xx from the Messages API. Body is the raw error envelope.
    #[error("api returned {status}: {body}")]
    Api { status: u16, body: String },

    /// An `event: error` frame arrived mid-stream (e.g. `overloaded_error`),
    /// or the stream was malformed.
    ///
    /// A `kind` beginning with [`PROVIDER_MESSAGE_PREFIX`] promises that `message`
    /// was written by the provider *for the user* and may be shown verbatim.
    #[error("stream {kind}: {message}")]
    Stream { kind: String, message: String },

    /// The turn ended for a reason the caller has to decide about:
    /// `refusal`, `max_tokens`, or an unrecognized stop reason.
    #[error("turn stopped: {0}")]
    StoppedEarly(String),

    /// User (or session controller) cancelled the in-flight turn.
    #[error("turn cancelled")]
    Cancelled,

    /// SSE ended without `message_stop` — the turn is not transactional.
    #[error("stream ended before message_stop")]
    PrematureEof,

    /// No bytes / events for longer than the idle budget.
    #[error("stream idle timeout")]
    StreamIdleTimeout,

    /// Retry gave up, wrapping the failure of the final attempt.
    ///
    /// A wrapper rather than a formatted string because the attempt count has to
    /// reach the user *and* the classification has to survive: `reqwest::Error`
    /// cannot be rebuilt with a note appended, so flattening it into text was
    /// discarding `is_connect()` — the one bit that tells "the gateway is not
    /// running" apart from "your session is bad".
    #[error("{source} (failed after {attempts} attempts)")]
    Exhausted {
        attempts: u32,
        source: Box<HarnessError>,
    },

    #[error("{0}")]
    Other(String),
}

/// Marks a [`HarnessError::Stream`] whose `message` is provider-authored user
/// text. An explicit prefix rather than a heuristic: `Other` and `Stream` also
/// carry internal strings, and those must never reach a chat bubble.
pub const PROVIDER_MESSAGE_PREFIX: &str = "provider:";

impl HarnessError {
    /// The failure underneath any retry wrapper.
    ///
    /// Every classifier below asks about the *original* failure, so none of them
    /// should have to know whether retry happened to give up on it.
    pub fn root(&self) -> &Self {
        match self {
            Self::Exhausted { source, .. } => source.root(),
            other => other,
        }
    }

    /// A message the provider wrote for the user, safe to show as-is.
    ///
    /// The bug this exists for: Codex reports `"You've hit your usage limit ... try
    /// again at <date>"`, which names both the cause and the fix, and the desktop
    /// replaced it with `"The provider could not complete the request."` — every
    /// actionable word thrown away because no classifier matched.
    pub fn provider_user_message(&self) -> Option<&str> {
        match self.root() {
            Self::Stream { kind, message }
                if kind.starts_with(PROVIDER_MESSAGE_PREFIX) && !message.trim().is_empty() =>
            {
                Some(message)
            }
            _ => None,
        }
    }

    /// The `error.message` a JSON API body carried, if it is short enough to
    /// show. ChatGPT's Codex backend and OpenAI-compatible endpoints put the
    /// real reason here; without this the desktop can only say "try again".
    pub fn provider_api_message(&self) -> Option<String> {
        let Self::Api { body, .. } = self.root() else {
            return None;
        };
        extract_api_error_message(body)
    }

    /// A mid-stream error whose `message` was written by the provider.
    pub fn from_provider_stream(kind: &str, message: impl Into<String>) -> Self {
        let message = message.into();
        let trimmed = message.trim();
        if trimmed.is_empty() {
            return Self::Stream {
                kind: kind.to_string(),
                message: "provider stream failed".into(),
            };
        }
        let kind = kind.trim();
        let kind = if kind.is_empty() { "error" } else { kind };
        Self::Stream {
            kind: format!("{PROVIDER_MESSAGE_PREFIX}{kind}"),
            message: trimmed.to_string(),
        }
    }

    /// Whether the request never reached a server: DNS, TCP connect, TLS, or a
    /// timeout with no response.
    ///
    /// Kept apart from [`Self::is_auth_problem`] because the two need opposite
    /// advice. Nothing is listening on the gateway's port is fixed by starting
    /// the gateway; signing in again cannot help, and telling someone to
    /// reconnect sends them through an OAuth flow that changes nothing.
    pub fn is_unreachable(&self) -> bool {
        matches!(self.root(), Self::Http(e) if e.is_connect() || e.is_timeout())
    }

    /// Whether this failure means the provider's credentials need renewing.
    ///
    /// The distinction that matters: a rate limit will pass on its own, a bad
    /// request needs a code change, but *this* class only clears when someone
    /// signs in again — so it is the only one worth putting a Reconnect button
    /// on. A gateway is the usual source, and it reports the problem in the
    /// body rather than the status: CLIProxyAPI answers 503 `auth_unavailable`
    /// for an account it holds but cannot use, which is indistinguishable from
    /// "temporarily overloaded" unless the body is read.
    pub fn is_auth_problem(&self) -> bool {
        let Self::Api { status, body } = self.root() else {
            return false;
        };
        if matches!(status, 401 | 403) {
            return true;
        }
        let body = body.to_ascii_lowercase();
        [
            "auth_unavailable",
            "authentication_error",
            "invalid_api_key",
            "no auth available",
            "unauthorized",
            "invalid x-api-key",
        ]
        .iter()
        .any(|needle| body.contains(needle))
    }

    /// Whether the provider rejected the request because its context window
    /// was exceeded. This is intentionally separate from ordinary bad-request
    /// errors so front-ends can tell the user to compact instead of retrying
    /// the same payload unchanged.
    pub fn is_context_limit(&self) -> bool {
        let Self::Api { status, body } = self.root() else {
            return false;
        };
        if *status == 413 {
            return true;
        }
        let body = body.to_ascii_lowercase();
        [
            "context length",
            "context window",
            "maximum context",
            "prompt is too long",
            "input is too long",
            "too many tokens",
            "token limit",
        ]
        .iter()
        .any(|needle| body.contains(needle))
    }

    /// Whether a failure that happened **before any output streamed** is worth
    /// another attempt.
    ///
    /// Deliberately narrow. Once bytes have reached the caller a retry would
    /// duplicate them, so this is only ever consulted on the request itself.
    /// 529 is Anthropic's overloaded signal; 429 is rate limiting; the rest are
    /// ordinary gateway flapping.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Http(e) => e.is_timeout() || e.is_connect(),
            Self::Api { status, .. } => matches!(status, 408 | 429 | 500 | 502 | 503 | 529),
            // Deliberately not delegating to the inner error. The attempts are
            // already spent; reporting "retryable" here would invite an outer
            // loop to spend them again.
            Self::Exhausted { .. } => false,
            _ => false,
        }
    }
}

fn extract_api_error_message(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let message = value
        .pointer("/error/message")
        .or_else(|| value.pointer("/response/error/message"))
        .or_else(|| value.pointer("/message"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|message| !message.is_empty())?;
    if message.chars().count() > 400 {
        let cut: String = message.chars().take(400).collect();
        return Some(format!("{cut}…"));
    }
    Some(message.to_string())
}

pub type Result<T> = std::result::Result<T, HarnessError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_classification_is_narrow() {
        for status in [408, 429, 500, 502, 503, 529] {
            assert!(
                HarnessError::Api {
                    status,
                    body: String::new()
                }
                .is_transient(),
                "{status} should retry"
            );
        }
        // A bad request or a bad key will fail identically every time.
        for status in [400, 401, 403, 404, 413, 422] {
            assert!(
                !HarnessError::Api {
                    status,
                    body: String::new()
                }
                .is_transient(),
                "{status} must not retry"
            );
        }
        assert!(!HarnessError::Cancelled.is_transient());
        // A 503 is retryable *and* can be an auth problem; the two questions
        // are independent, and the body is what tells them apart.
        assert!(HarnessError::Api {
            status: 503,
            body: r#"{"error":{"message":"auth_unavailable: no auth available"}}"#.into()
        }
        .is_transient());
        assert!(!HarnessError::PrematureEof.is_transient());
        // Mid-stream failures must never retry — output already reached the UI.
        assert!(!HarnessError::Stream {
            kind: "overloaded_error".into(),
            message: "busy".into()
        }
        .is_transient());
    }

    /// The prefix is a promise, not a guess: an ordinary `Stream` or `Other`
    /// carries internal text that must never be rendered as a chat message.
    #[test]
    fn only_an_explicitly_tagged_message_is_offered_to_the_ui() {
        let tagged = HarnessError::Stream {
            kind: "provider:usageLimitExceeded".into(),
            message: "You've hit your usage limit.".into(),
        };
        assert_eq!(
            tagged.provider_user_message(),
            Some("You've hit your usage limit.")
        );

        let internal = HarnessError::Stream {
            kind: "codex_protocol".into(),
            message: "thread/start returned no thread id".into(),
        };
        assert_eq!(internal.provider_user_message(), None);
        assert_eq!(
            HarnessError::Other("could not read credential".into()).provider_user_message(),
            None
        );

        // Blank text is worse than the generic sentence it would replace.
        let blank = HarnessError::Stream {
            kind: "provider:codex".into(),
            message: "   ".into(),
        };
        assert_eq!(blank.provider_user_message(), None);
    }

    #[test]
    fn an_openai_api_body_offers_its_error_message() {
        let failure = HarnessError::Api {
            status: 402,
            body: r#"{"error":{"message":"Insufficient Balance","type":"unknown_error"}}"#.into(),
        };
        assert_eq!(
            failure.provider_api_message().as_deref(),
            Some("Insufficient Balance")
        );
        assert_eq!(extract_api_error_message("not json"), None);
    }

    #[test]
    fn a_chatgpt_responses_body_offers_its_error_message() {
        let failure = HarnessError::Api {
            status: 429,
            body: r#"{"error":{"message":"You've hit your usage limit. Try again in 3 hours.","type":"usage_limit_exceeded"}}"#.into(),
        };
        assert_eq!(
            failure.provider_api_message().as_deref(),
            Some("You've hit your usage limit. Try again in 3 hours.")
        );

        let nested = HarnessError::Api {
            status: 400,
            body:
                r#"{"response":{"error":{"message":"This model is not available on your plan."}}}"#
                    .into(),
        };
        assert_eq!(
            nested.provider_api_message().as_deref(),
            Some("This model is not available on your plan.")
        );
    }

    #[test]
    fn a_provider_stream_error_is_tagged_only_when_the_message_is_theirs() {
        let tagged = HarnessError::from_provider_stream(
            "usage_limit_exceeded",
            "You've hit your usage limit.",
        );
        assert_eq!(
            tagged.provider_user_message(),
            Some("You've hit your usage limit.")
        );

        let blank = HarnessError::from_provider_stream("error", "   ");
        assert_eq!(blank.provider_user_message(), None);
        assert!(matches!(
            blank,
            HarnessError::Stream { kind, .. } if kind == "error"
        ));
    }

    /// Retry must not hide the tag, or a single retried attempt would silently
    /// downgrade the message the user sees.
    #[test]
    fn a_provider_message_survives_the_retry_wrapper() {
        let wrapped = HarnessError::Exhausted {
            attempts: 3,
            source: Box::new(HarnessError::Stream {
                kind: "provider:usageLimitExceeded".into(),
                message: "try again tomorrow".into(),
            }),
        };
        assert_eq!(wrapped.provider_user_message(), Some("try again tomorrow"));
    }

    #[test]
    fn auth_problems_are_recognised_by_body_not_just_status() {
        // The one that actually happened: CLIProxyAPI holds a Claude session it
        // cannot use and reports 503, which looks like ordinary overload.
        let real = HarnessError::Api {
            status: 503,
            body: r#"{"type":"error","error":{"type":"api_error","message":"auth_unavailable: no auth available (providers=claude, model=claude-opus-5); check Claude auth/key session and cooldown state"}}"#.into(),
        };
        assert!(real.is_auth_problem());

        for (status, body) in [
            (401u16, "{}"),
            (403, "{}"),
            (400, r#"{"error":{"type":"authentication_error"}}"#),
            (400, r#"{"error":{"message":"invalid x-api-key"}}"#),
        ] {
            assert!(
                HarnessError::Api {
                    status,
                    body: body.into()
                }
                .is_auth_problem(),
                "{status} {body}"
            );
        }
    }

    #[test]
    fn ordinary_failures_do_not_offer_a_reconnect() {
        // Signing in again fixes none of these, so suggesting it would send the
        // user through an OAuth flow for nothing.
        for (status, body) in [
            (503u16, r#"{"error":{"message":"overloaded_error"}}"#),
            (429, r#"{"error":{"message":"rate_limit_error"}}"#),
            (400, r#"{"error":{"message":"max_tokens is too large"}}"#),
            (404, r#"{"error":{"message":"model not found"}}"#),
        ] {
            assert!(
                !HarnessError::Api {
                    status,
                    body: body.into()
                }
                .is_auth_problem(),
                "{status} {body}"
            );
        }
        assert!(!HarnessError::Cancelled.is_auth_problem());
        assert!(!HarnessError::StreamIdleTimeout.is_auth_problem());
    }

    #[test]
    fn context_limits_are_classified_separately() {
        for (status, body) in [
            (
                400u16,
                r#"{"error":{"message":"maximum context length is 128k"}}"#,
            ),
            (413, r#"{"error":{"message":"payload too large"}}"#),
            (422, r#"{"error":{"message":"prompt is too long"}}"#),
        ] {
            assert!(
                HarnessError::Api {
                    status,
                    body: body.into()
                }
                .is_context_limit(),
                "{status} {body}"
            );
        }
        assert!(!HarnessError::Api {
            status: 400,
            body: r#"{"error":{"message":"invalid model"}}"#.into()
        }
        .is_context_limit());
    }

    /// The regression that made this variant necessary: three failed attempts
    /// used to turn a 503 `auth_unavailable` into an unparseable body, and a
    /// refused connection into a string that no longer looked like a transport
    /// failure at all. Both classifiers must see through the wrapper.
    #[test]
    fn giving_up_does_not_change_what_the_failure_was() {
        let auth = HarnessError::Exhausted {
            attempts: 3,
            source: Box::new(HarnessError::Api {
                status: 503,
                body: r#"{"error":{"message":"auth_unavailable: no auth available"}}"#.into(),
            }),
        };
        assert!(
            auth.is_auth_problem(),
            "still an auth problem after retries"
        );
        assert!(!auth.is_unreachable(), "a served 503 did reach a server");
        // The attempt count still reaches the user.
        assert!(
            auth.to_string().contains("failed after 3 attempts"),
            "{auth}"
        );
        // And the body stays valid JSON, which `api_error_message` parses.
        assert!(matches!(auth.root(), HarnessError::Api { .. }));

        // An exhausted retry is not itself retryable — the attempts are spent.
        assert!(!auth.is_transient());
    }

    #[test]
    fn an_unreachable_endpoint_is_not_an_auth_problem() {
        // Signing in again cannot make a dead port answer, so these two must
        // never be confused: they carry opposite advice.
        let refused = HarnessError::Exhausted {
            attempts: 3,
            source: Box::new(HarnessError::Other("connection refused".into())),
        };
        assert!(!refused.is_auth_problem());
    }
}
