//! Does a CLI's stream still look the way we parsed it for?
//!
//! Every provider stream is parsed against an undocumented, versioned schema
//! that its vendor is free to change. When it does change, a hand-written
//! parser does not fail: it silently stops recognising a record type and the
//! feature it fed quietly disappears. That failure mode is invisible, which is
//! the reason to measure it.
//!
//! Ported from `anyagentcli-mcp`'s `providers/contract.ts`. A normalizer counts
//! the record kinds it saw, and at the end of a turn the counts are checked
//! against a declared spec: records that had to appear, and records we know how
//! to handle. Anything else is drift.
//!
//! This is a *report*, never a hard failure. A stream that grew a new record
//! type is still a working turn, and refusing to finish it because a vendor
//! shipped a feature would be worse than the drift.
//!
//! The [`StreamNormalizer`] seam lives here too, as the provider-neutral half
//! of reading a stream: the runner needs a way to hand records to whoever knows
//! the schema, without learning any schema itself.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::Value;

use crate::tools::external_agent::ExternalAgentEvent;

/// Turns one provider's stream records into Zest events.
///
/// The trait exists so the runner can stay ignorant of which CLI it is reading.
/// `cursor_acp` and `codex_app_server` have no implementation yet and still go
/// through the schema-agnostic path; writing one is how they stop needing it.
pub trait StreamNormalizer: Send {
    /// Events for one parsed record. An empty result is normal: most records
    /// carry state rather than something to show.
    fn normalize(&mut self, raw: &Value) -> Vec<ExternalAgentEvent>;

    /// Note a line that was not valid JSON, for the contract report.
    fn parse_error(&mut self);

    fn report(&self, outcome: RunOutcome) -> ContractReport;
}

/// How a turn ended, recorded alongside the counts.
///
/// Drift matters differently depending on the ending: unknown records in a
/// successful turn are a feature we ignore, and unknown records in a failed one
/// are a candidate explanation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Success,
    Error,
    Timeout,
    Abort,
    Unknown,
}

impl fmt::Display for RunOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Success => "success",
            Self::Error => "error",
            Self::Timeout => "timeout",
            Self::Abort => "abort",
            Self::Unknown => "unknown",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractStatus {
    /// Everything required arrived and nothing unrecognised did.
    Ok,
    /// Everything required arrived, but the stream carried records we ignore.
    Degraded,
    /// A record the parser depends on never arrived.
    Violated,
}

impl fmt::Display for ContractStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Ok => "ok",
            Self::Degraded => "degraded",
            Self::Violated => "violated",
        })
    }
}

/// What a provider's stream must and may contain.
///
/// Discriminants are strings rather than an enum because they are compound:
/// `system:init` and `assistant:tool_use` name a record *and* the field inside
/// it that decides the meaning. An enum would have to be re-shaped for every
/// provider added.
#[derive(Debug, Clone, Copy)]
pub struct ContractSpec {
    pub provider: &'static str,
    /// Absent from a finished turn means the parser lost something it needs.
    pub required: &'static [&'static str],
    /// Everything the normalizer recognises, required entries included.
    pub known: &'static [&'static str],
}

/// Claude Code's `--output-format stream-json`, as of CLI 2.1.220.
///
/// `system:init` carries the session id and the resolved tool list, and
/// `result` carries the final answer. Losing either is not a degraded turn, it
/// is a broken one.
pub const CLAUDE_CODE_CONTRACT: ContractSpec = ContractSpec {
    provider: "claude_code",
    required: &["system:init", "result"],
    known: &[
        "system:init",
        // Progress chatter with nothing to show: `thinking_tokens` is a running
        // estimate and `status` is the request lifecycle (`"requesting"`).
        // Listed so they do not read as drift; both are found on every turn.
        "system:thinking_tokens",
        "system:status",
        "rate_limit_event",
        "stream_event:text_delta",
        "stream_event:thinking_delta",
        "assistant:text",
        "assistant:thinking",
        "assistant:tool_use",
        "user:tool_result",
        "result",
    ],
};

// Contracts for `cursor_acp` and `codex_app_server` belong here too, but are
// deliberately absent: neither has a normalizer yet, and a spec with nothing
// counting against it would assert a shape nobody has checked.

/// A discriminant the spec required and the stream never produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractViolation {
    pub expected: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractReport {
    pub provider: &'static str,
    pub outcome: RunOutcome,
    pub status: ContractStatus,
    pub counts: BTreeMap<String, u64>,
    pub unknown: BTreeMap<String, u64>,
    pub violations: Vec<ContractViolation>,
    pub raw_lines: u64,
    pub parse_errors: u64,
}

impl ContractReport {
    /// A one-line summary for a log. `None` when there is nothing to say.
    pub fn describe(&self) -> Option<String> {
        if self.status == ContractStatus::Ok {
            return None;
        }
        let mut parts = Vec::new();
        if !self.violations.is_empty() {
            let missing: Vec<&str> = self
                .violations
                .iter()
                .map(|violation| violation.expected.as_str())
                .collect();
            parts.push(format!("missing: {}", missing.join(", ")));
        }
        if !self.unknown.is_empty() {
            let unknown: Vec<&str> = self.unknown.keys().map(String::as_str).collect();
            parts.push(format!("unknown: {}", unknown.join(", ")));
        }
        Some(format!(
            "{} stream {} on a {} turn ({})",
            self.provider,
            self.status,
            self.outcome,
            parts.join(" | ")
        ))
    }
}

/// Caps on what a report may hold.
///
/// The stream is written by the child process, so the number of distinct
/// discriminants is not ours to choose. Without a ceiling a malformed or
/// hostile stream could name a million record types and the report would hold
/// all of them.
const MAX_TRACKED: usize = 100;
const MAX_UNKNOWN: usize = 50;
const MAX_KEY_CHARS: usize = 40;

/// Running tally of what a stream contained. Normalizers own one.
#[derive(Debug, Clone, Default)]
pub struct ContractLedger {
    counts: BTreeMap<String, u64>,
    unknown: BTreeMap<String, u64>,
    raw_lines: u64,
    parse_errors: u64,
}

impl ContractLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one discriminant, classifying it against the spec.
    pub fn observe(&mut self, spec: &ContractSpec, discriminant: &str) {
        if spec.known.contains(&discriminant) {
            bump(&mut self.counts, discriminant, MAX_TRACKED);
        } else {
            bump(&mut self.unknown, discriminant, MAX_UNKNOWN);
        }
    }

    pub fn line(&mut self) {
        self.raw_lines = self.raw_lines.saturating_add(1);
    }

    pub fn parse_error(&mut self) {
        self.parse_errors = self.parse_errors.saturating_add(1);
    }

    pub fn report(&self, spec: &ContractSpec, outcome: RunOutcome) -> ContractReport {
        let violations: Vec<ContractViolation> = spec
            .required
            .iter()
            .filter(|required| !self.counts.contains_key(**required))
            .map(|required| ContractViolation {
                expected: (*required).to_string(),
            })
            .collect();

        let status = if !violations.is_empty() {
            ContractStatus::Violated
        } else if !self.unknown.is_empty() {
            ContractStatus::Degraded
        } else {
            ContractStatus::Ok
        };

        ContractReport {
            provider: spec.provider,
            outcome,
            status,
            counts: self.counts.clone(),
            unknown: self.unknown.clone(),
            violations,
            raw_lines: self.raw_lines,
            parse_errors: self.parse_errors,
        }
    }
}

/// Increment a key, refusing to grow the map past `limit`.
///
/// A key already present is always counted, so a capped map keeps measuring
/// what it already knows instead of freezing wholesale.
fn bump(map: &mut BTreeMap<String, u64>, key: &str, limit: usize) {
    if let Some(count) = map.get_mut(key) {
        *count = count.saturating_add(1);
        return;
    }
    if map.len() >= limit {
        return;
    }
    let key: String = key.chars().take(MAX_KEY_CHARS).collect();
    map.insert(key, 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_complete_stream_reports_ok() {
        let mut ledger = ContractLedger::new();
        for discriminant in ["system:init", "assistant:text", "result"] {
            ledger.observe(&CLAUDE_CODE_CONTRACT, discriminant);
        }
        let report = ledger.report(&CLAUDE_CODE_CONTRACT, RunOutcome::Success);

        assert_eq!(report.status, ContractStatus::Ok);
        assert!(report.violations.is_empty());
        assert_eq!(report.describe(), None);
    }

    #[test]
    fn a_missing_required_record_is_a_violation() {
        let mut ledger = ContractLedger::new();
        ledger.observe(&CLAUDE_CODE_CONTRACT, "system:init");
        let report = ledger.report(&CLAUDE_CODE_CONTRACT, RunOutcome::Error);

        assert_eq!(report.status, ContractStatus::Violated);
        assert_eq!(report.violations.len(), 1);
        assert_eq!(report.violations[0].expected, "result");
        assert!(report.describe().unwrap().contains("missing: result"));
    }

    #[test]
    fn an_unrecognised_record_degrades_without_failing() {
        let mut ledger = ContractLedger::new();
        for discriminant in ["system:init", "result", "system:brand_new"] {
            ledger.observe(&CLAUDE_CODE_CONTRACT, discriminant);
        }
        let report = ledger.report(&CLAUDE_CODE_CONTRACT, RunOutcome::Success);

        assert_eq!(report.status, ContractStatus::Degraded);
        assert!(report.violations.is_empty());
        assert_eq!(report.unknown.get("system:brand_new"), Some(&1));
        assert!(report
            .describe()
            .unwrap()
            .contains("unknown: system:brand_new"));
    }

    /// The subtype that made this worth building: it is on every live stream
    /// today and nothing in Zest counted it.
    #[test]
    fn thinking_tokens_is_a_known_record_not_drift() {
        let mut ledger = ContractLedger::new();
        for discriminant in ["system:init", "system:thinking_tokens", "result"] {
            ledger.observe(&CLAUDE_CODE_CONTRACT, discriminant);
        }
        let report = ledger.report(&CLAUDE_CODE_CONTRACT, RunOutcome::Success);
        assert_eq!(report.status, ContractStatus::Ok);
        assert_eq!(report.counts.get("system:thinking_tokens"), Some(&1));
    }

    /// The child process picks the discriminants, so the report must not grow
    /// without bound when the child picks badly.
    #[test]
    fn unknown_discriminants_are_capped() {
        let mut ledger = ContractLedger::new();
        for index in 0..(MAX_UNKNOWN * 3) {
            ledger.observe(&CLAUDE_CODE_CONTRACT, &format!("junk:{index}"));
        }
        // Already-tracked keys keep counting even once the map is full.
        ledger.observe(&CLAUDE_CODE_CONTRACT, "junk:0");

        let report = ledger.report(&CLAUDE_CODE_CONTRACT, RunOutcome::Unknown);
        assert_eq!(report.unknown.len(), MAX_UNKNOWN);
        assert_eq!(report.unknown.get("junk:0"), Some(&2));
    }

    #[test]
    fn long_discriminants_are_truncated() {
        let mut ledger = ContractLedger::new();
        ledger.observe(&CLAUDE_CODE_CONTRACT, &"x".repeat(500));
        let report = ledger.report(&CLAUDE_CODE_CONTRACT, RunOutcome::Unknown);
        assert_eq!(report.unknown.keys().next().unwrap().len(), MAX_KEY_CHARS);
    }
}
