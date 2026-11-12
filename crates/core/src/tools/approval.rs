//! Minimal tool approval gate.
//!
//! Read tools run without prompting. Write/exec/sensitive-read tools pause for
//! an [`Approver`] decision before execution. Decisions are session-scoped —
//! nothing is persisted to disk here.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// How dangerous a tool invocation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRisk {
    Read,
    /// Explicit read of a likely-secret file — requires per-call approval.
    Sensitive,
    Write,
    Exec,
}

impl ToolRisk {
    pub fn requires_approval(self) -> bool {
        matches!(self, Self::Sensitive | Self::Write | Self::Exec)
    }
}

/// What the UI (or CLI) should show before a gated tool runs.
#[derive(Debug, Clone)]
pub struct ApprovalPreview {
    pub path: String,
    pub summary: String,
    pub diff: String,
}

/// Request handed to an [`Approver`].
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub approval_id: String,
    pub tool_name: String,
    pub tool_call_id: String,
    pub risk: ToolRisk,
    pub preview: ApprovalPreview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    AllowOnce,
    /// Allow this call and stop asking about the same tool **and target** for
    /// the rest of the session. Deliberately not "the same tool": the user
    /// approved a specific diff or a specific command, and a blanket grant
    /// would not be the thing they were shown.
    AllowSession,
    Deny,
}

/// How much the harness may do without asking.
///
/// The gate itself never changes — every write and command still passes through
/// it. The mode only decides whether a human is consulted at that point, which
/// keeps one code path for "may this run" instead of several.
/// The derived default is [`Manual`](ApprovalMode::Manual), not `Auto`. A bare
/// [`Agent`](crate::Agent) also defaults to the deny-all approver, and those two
/// defaults have to agree — a permissive policy would let writes through
/// *because* no front-end had wired a gate yet, which is exactly backwards.
/// Front-ends pick their own startup mode; the desktop chooses `Auto`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// Ask before every write and every command, including read-only ones.
    #[default]
    Manual,
    /// Writes go through; every command is still confirmed.
    AcceptEdits,
    /// Read and research only. Writes and commands are refused outright rather
    /// than queued for approval, so the model plans instead of stalling on a
    /// card the user does not want to click.
    Plan,
    /// Writes go through, and so do commands on the read-only allowlist.
    /// Anything else asks. The daily driver.
    Auto,
    /// Nothing asks. For a sandbox or a throwaway tree.
    Bypass,
}

impl ApprovalMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::AcceptEdits => "accept_edits",
            Self::Plan => "plan",
            Self::Auto => "auto",
            Self::Bypass => "bypass",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw
            .trim()
            .to_ascii_lowercase()
            .replace(['-', ' '], "_")
            .as_str()
        {
            "manual" => Some(Self::Manual),
            "accept_edits" | "acceptedits" | "edits" => Some(Self::AcceptEdits),
            "plan" => Some(Self::Plan),
            "auto" => Some(Self::Auto),
            "bypass" | "bypass_permissions" => Some(Self::Bypass),
            _ => None,
        }
    }

    /// Human label for the picker and for refusal messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::Manual => "Manual",
            Self::AcceptEdits => "Accept edits",
            Self::Plan => "Plan",
            Self::Auto => "Auto",
            Self::Bypass => "Bypass permissions",
        }
    }
}

/// What the policy decided before any human was involved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyOutcome {
    /// Run it without asking.
    Allow,
    /// Refuse without asking. Carries the reason the model will see.
    Block(String),
    /// The policy has no opinion — put it to the user.
    Ask,
}

/// Session-scoped permission state: the current mode plus whatever the user has
/// already said "allow for the session" to.
///
/// Nothing here is persisted. A grant lasts as long as the process, so
/// reopening the app is always a clean slate.
#[derive(Debug, Default)]
pub struct ApprovalPolicy {
    mode: ApprovalMode,
    /// `(tool_name, target)` pairs the user has trusted for this session.
    /// Target `"*"` means every invocation of that tool.
    trusted: std::collections::HashSet<(String, String)>,
}

/// Session grant that covers every target for a tool. Used when the user
/// clicked "Allow for session" on a command-class Claude tool (Read, Bash,
/// WebFetch) — the next card is the same tool with a different string.
const ANY_TARGET: &str = "*";

impl ApprovalPolicy {
    pub fn new(mode: ApprovalMode) -> Self {
        Self {
            mode,
            trusted: std::collections::HashSet::new(),
        }
    }

    pub fn mode(&self) -> ApprovalMode {
        self.mode
    }

    /// Switching mode clears session grants.
    ///
    /// A grant is consent given under one policy; carrying it into a stricter
    /// mode would mean tightening the setting and getting the old permissions
    /// anyway. Dropping them is the interpretation that cannot surprise anyone.
    pub fn set_mode(&mut self, mode: ApprovalMode) {
        if mode != self.mode {
            self.trusted.clear();
        }
        self.mode = mode;
    }

    pub fn trust(&mut self, tool_name: &str, target: &str) {
        self.trusted
            .insert((tool_name.to_string(), target.to_string()));
    }

    pub fn trust_tool(&mut self, tool_name: &str) {
        self.trust(tool_name, ANY_TARGET);
    }

    pub fn is_trusted(&self, tool_name: &str, target: &str) -> bool {
        self.trusted
            .contains(&(tool_name.to_string(), target.to_string()))
            || self
                .trusted
                .contains(&(tool_name.to_string(), ANY_TARGET.to_string()))
    }

    /// Decide what happens to a gated call before a human sees it.
    ///
    /// `auto_eligible` means the tool itself vouched for this specific
    /// invocation as read-only — today only `bash` sets it, for commands that
    /// match the allowlist and contain no shell metacharacters.
    pub fn decide(
        &self,
        tool_name: &str,
        target: &str,
        risk: ToolRisk,
        auto_eligible: bool,
    ) -> PolicyOutcome {
        if !risk.requires_approval() {
            return PolicyOutcome::Allow;
        }

        // An explicit session grant outranks the mode for that one target,
        // except in Plan mode where nothing may write at all.
        let trusted = self.is_trusted(tool_name, target);

        match self.mode {
            ApprovalMode::Bypass => PolicyOutcome::Allow,

            ApprovalMode::Plan => PolicyOutcome::Block(format!(
                "Plan mode is on, so `{tool_name}` cannot run. Describe what you \
                 would change and why; the user will switch modes to apply it."
            )),

            ApprovalMode::Manual => {
                if trusted {
                    PolicyOutcome::Allow
                } else {
                    PolicyOutcome::Ask
                }
            }

            ApprovalMode::AcceptEdits => match risk {
                ToolRisk::Write => PolicyOutcome::Allow,
                _ if trusted => PolicyOutcome::Allow,
                _ => PolicyOutcome::Ask,
            },

            ApprovalMode::Auto => match risk {
                ToolRisk::Write => PolicyOutcome::Allow,
                ToolRisk::Exec if auto_eligible => PolicyOutcome::Allow,
                _ if trusted => PolicyOutcome::Allow,
                _ => PolicyOutcome::Ask,
            },
        }
    }
}

/// Front-end hook: desktop waits on the user; CLI may auto-deny or prompt.
#[async_trait]
pub trait Approver: Send + Sync {
    /// Reserve a wait slot **before** `ApprovalNeeded` is emitted so a fast
    /// UI click cannot race the registration.
    async fn prepare(&self, _approval_id: &str) {}

    async fn decide(&self, request: &ApprovalRequest) -> ApprovalDecision;
}

/// Safe default when no front-end is wired — deny every gated call.
pub struct DenyApprover;

#[async_trait]
impl Approver for DenyApprover {
    async fn decide(&self, _request: &ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::Deny
    }
}

/// Test helper that allows every gated call.
pub struct AllowApprover;

#[async_trait]
impl Approver for AllowApprover {
    async fn decide(&self, _request: &ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::AllowOnce
    }
}

#[cfg(test)]
mod characterization {
    use super::*;

    #[test]
    fn tool_risk_approval_defaults() {
        assert!(!ToolRisk::Read.requires_approval());
        assert!(ToolRisk::Sensitive.requires_approval());
        assert!(ToolRisk::Write.requires_approval());
        assert!(ToolRisk::Exec.requires_approval());
    }

    #[test]
    fn tool_risk_serde_snake_case() {
        assert_eq!(
            serde_json::to_string(&ToolRisk::Write).unwrap(),
            "\"write\""
        );
        assert_eq!(
            serde_json::from_str::<ToolRisk>("\"sensitive\"").unwrap(),
            ToolRisk::Sensitive
        );
        assert_eq!(
            serde_json::from_str::<ToolRisk>("\"exec\"").unwrap(),
            ToolRisk::Exec
        );
    }

    fn decide(mode: ApprovalMode, risk: ToolRisk, auto_eligible: bool) -> PolicyOutcome {
        ApprovalPolicy::new(mode).decide("bash", "cargo check", risk, auto_eligible)
    }

    #[test]
    fn the_library_default_is_the_strict_mode() {
        // Pairs with DenyApprover: an un-wired Agent must not write.
        assert_eq!(ApprovalMode::default(), ApprovalMode::Manual);
        let policy = ApprovalPolicy::default();
        assert_eq!(
            policy.decide("write_file", "a.rs", ToolRisk::Write, false),
            PolicyOutcome::Ask
        );
    }

    #[test]
    fn read_risk_never_reaches_the_gate_in_any_mode() {
        for mode in [
            ApprovalMode::Manual,
            ApprovalMode::AcceptEdits,
            ApprovalMode::Plan,
            ApprovalMode::Auto,
            ApprovalMode::Bypass,
        ] {
            assert_eq!(
                decide(mode, ToolRisk::Read, false),
                PolicyOutcome::Allow,
                "{mode:?}"
            );
        }
    }

    #[test]
    fn manual_asks_for_everything_including_safe_commands() {
        assert_eq!(
            decide(ApprovalMode::Manual, ToolRisk::Write, false),
            PolicyOutcome::Ask
        );
        assert_eq!(
            decide(ApprovalMode::Manual, ToolRisk::Exec, false),
            PolicyOutcome::Ask
        );
        // Manual means manual: an allowlisted command is still confirmed.
        assert_eq!(
            decide(ApprovalMode::Manual, ToolRisk::Exec, true),
            PolicyOutcome::Ask
        );
    }

    #[test]
    fn accept_edits_passes_writes_but_still_confirms_every_command() {
        assert_eq!(
            decide(ApprovalMode::AcceptEdits, ToolRisk::Write, false),
            PolicyOutcome::Allow
        );
        assert_eq!(
            decide(ApprovalMode::AcceptEdits, ToolRisk::Exec, false),
            PolicyOutcome::Ask
        );
        assert_eq!(
            decide(ApprovalMode::AcceptEdits, ToolRisk::Exec, true),
            PolicyOutcome::Ask,
            "accept-edits is the cautious-about-shell mode"
        );
        // A sensitive read is not an edit.
        assert_eq!(
            decide(ApprovalMode::AcceptEdits, ToolRisk::Sensitive, false),
            PolicyOutcome::Ask
        );
    }

    #[test]
    fn auto_passes_writes_and_allowlisted_commands_only() {
        assert_eq!(
            decide(ApprovalMode::Auto, ToolRisk::Write, false),
            PolicyOutcome::Allow
        );
        assert_eq!(
            decide(ApprovalMode::Auto, ToolRisk::Exec, true),
            PolicyOutcome::Allow
        );
        assert_eq!(
            decide(ApprovalMode::Auto, ToolRisk::Exec, false),
            PolicyOutcome::Ask,
            "an unlisted command must still be confirmed in Auto"
        );
        assert_eq!(
            decide(ApprovalMode::Auto, ToolRisk::Sensitive, false),
            PolicyOutcome::Ask
        );
    }

    #[test]
    fn plan_refuses_rather_than_queueing_a_card() {
        for (risk, eligible) in [
            (ToolRisk::Write, false),
            (ToolRisk::Exec, false),
            (ToolRisk::Exec, true),
            (ToolRisk::Sensitive, false),
        ] {
            match decide(ApprovalMode::Plan, risk, eligible) {
                PolicyOutcome::Block(reason) => {
                    assert!(reason.contains("Plan mode"), "{reason}");
                }
                other => panic!("expected Block for {risk:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn plan_mode_outranks_a_session_grant() {
        // Switching into Plan is a stronger statement than an earlier "allow
        // for session", and it must not be quietly overridden by one.
        let mut policy = ApprovalPolicy::new(ApprovalMode::Auto);
        policy.trust("bash", "rm -rf target");
        policy.set_mode(ApprovalMode::Plan);
        assert!(matches!(
            policy.decide("bash", "rm -rf target", ToolRisk::Exec, false),
            PolicyOutcome::Block(_)
        ));
    }

    #[test]
    fn bypass_allows_everything() {
        for risk in [ToolRisk::Write, ToolRisk::Exec, ToolRisk::Sensitive] {
            assert_eq!(
                decide(ApprovalMode::Bypass, risk, false),
                PolicyOutcome::Allow
            );
        }
    }

    #[test]
    fn a_session_grant_is_scoped_to_one_target() {
        let mut policy = ApprovalPolicy::new(ApprovalMode::Manual);
        policy.trust("bash", "npm install");

        assert_eq!(
            policy.decide("bash", "npm install", ToolRisk::Exec, false),
            PolicyOutcome::Allow
        );
        // A different command was never shown to the user.
        assert_eq!(
            policy.decide("bash", "npm publish", ToolRisk::Exec, false),
            PolicyOutcome::Ask
        );
        // Nor does it leak across tools.
        assert_eq!(
            policy.decide("write_file", "npm install", ToolRisk::Write, false),
            PolicyOutcome::Ask
        );
    }

    #[test]
    fn a_tool_wide_session_grant_covers_every_string() {
        let mut policy = ApprovalPolicy::new(ApprovalMode::Manual);
        policy.trust_tool("Read");
        assert_eq!(
            policy.decide("Read", "C:\\\\temp\\\\a.png", ToolRisk::Sensitive, false),
            PolicyOutcome::Allow
        );
        assert_eq!(
            policy.decide("Read", "C:\\\\temp\\\\b.png", ToolRisk::Sensitive, false),
            PolicyOutcome::Allow
        );
        assert_eq!(
            policy.decide("Bash", "echo hi", ToolRisk::Exec, false),
            PolicyOutcome::Ask
        );
    }

    #[test]
    fn changing_mode_drops_session_grants() {
        let mut policy = ApprovalPolicy::new(ApprovalMode::Manual);
        policy.trust("bash", "npm install");
        assert!(policy.is_trusted("bash", "npm install"));

        // Tightening the setting must not leave old consent in place.
        policy.set_mode(ApprovalMode::AcceptEdits);
        assert!(!policy.is_trusted("bash", "npm install"));
        assert_eq!(
            policy.decide("bash", "npm install", ToolRisk::Exec, false),
            PolicyOutcome::Ask
        );
    }

    #[test]
    fn setting_the_same_mode_keeps_grants() {
        let mut policy = ApprovalPolicy::new(ApprovalMode::Manual);
        policy.trust("bash", "npm install");
        policy.set_mode(ApprovalMode::Manual);
        assert!(policy.is_trusted("bash", "npm install"));
    }

    #[test]
    fn mode_round_trips_through_its_wire_name() {
        for mode in [
            ApprovalMode::Manual,
            ApprovalMode::AcceptEdits,
            ApprovalMode::Plan,
            ApprovalMode::Auto,
            ApprovalMode::Bypass,
        ] {
            assert_eq!(ApprovalMode::parse(mode.as_str()), Some(mode), "{mode:?}");
        }
        assert_eq!(
            ApprovalMode::parse("Accept Edits"),
            Some(ApprovalMode::AcceptEdits)
        );
        assert_eq!(
            ApprovalMode::parse("bypass-permissions"),
            Some(ApprovalMode::Bypass)
        );
        assert_eq!(ApprovalMode::parse("nonsense"), None);
    }

    #[tokio::test]
    async fn deny_approver_is_safe_default() {
        let decision = DenyApprover
            .decide(&ApprovalRequest {
                approval_id: "a1".into(),
                tool_name: "write_file".into(),
                tool_call_id: "t1".into(),
                risk: ToolRisk::Write,
                preview: ApprovalPreview {
                    path: "f.txt".into(),
                    summary: "write f.txt".into(),
                    diff: "".into(),
                },
            })
            .await;
        assert_eq!(decision, ApprovalDecision::Deny);
    }
}
