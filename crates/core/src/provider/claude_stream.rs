//! Claude Code's `stream-json` records, turned into Zest events.
//!
//! Zest used to read this stream through [`absorb_headless_value`], which
//! serves every headless CLI at once. Being provider-neutral is what makes it
//! useful for `delegate_external`, and also what makes it lossy here: it has no
//! way to know that `toolu_01…` in one record names the `Read` in another, so
//! every completed tool call rendered as `External tool`, and `system:init` —
//! the record carrying the session id — fell through to a default arm that
//! looked for a `response` field and found nothing.
//!
//! Ported from `anyagentcli-mcp`'s `providers/claude-code/normalize.ts`.
//!
//! Not ported: its sibling `filter.ts`, which strips records before they are
//! written to disk. Anygent persists every raw provider line; Zest does not
//! persist any, so that file solves a problem this codebase does not have.
//!
//! [`absorb_headless_value`]: crate::tools::external_agent

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::stream_contract::{
    ContractLedger, ContractReport, ContractSpec, RunOutcome, StreamNormalizer,
    CLAUDE_CODE_CONTRACT,
};
use crate::tools::external_agent::ExternalAgentEvent;

/// Tool activity Zest showed as a row, kept until its result arrives.
#[derive(Debug, Clone)]
struct PendingTool {
    name: String,
    title: String,
    path: Option<String>,
}

/// Tools whose successful completion changed a file on disk.
const EDIT_TOOLS: &[&str] = &["Edit", "Write", "NotebookEdit"];

/// How much of a subagent's message is shown on its parent's row.
const SUBAGENT_TITLE_CHARS: usize = 120;

pub struct ClaudeNormalizer {
    root: PathBuf,
    spec: &'static ContractSpec,
    ledger: ContractLedger,
    pending: HashMap<String, PendingTool>,
    /// Text assembled from partial-message deltas, used to decide whether the
    /// final result would repeat what the user already read.
    streamed_text: String,
    has_finish: bool,
    result_error: Option<String>,
    /// The session id Zest asked the CLI to use, when it asked for one.
    expected_session: Option<String>,
    session_id: Option<String>,
    session_mismatch: Option<String>,
    /// The tool names Zest put on `--tools`, and what the CLI actually resolved.
    requested_tools: Vec<String>,
    dropped_tools: Vec<String>,
}

impl ClaudeNormalizer {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            spec: &CLAUDE_CODE_CONTRACT,
            ledger: ContractLedger::new(),
            pending: HashMap::new(),
            streamed_text: String::new(),
            has_finish: false,
            result_error: None,
            expected_session: None,
            session_id: None,
            session_mismatch: None,
            requested_tools: Vec::new(),
            dropped_tools: Vec::new(),
        }
    }

    /// The session id Zest passed on `--session-id`, so the normalizer can
    /// confirm the CLI honoured it.
    pub fn expecting_session(mut self, session_id: impl Into<String>) -> Self {
        self.expected_session = Some(session_id.into());
        self
    }

    /// The `--tools` list Zest asked for.
    ///
    /// Worth recording because the CLI drops names it does not recognise
    /// *silently*: asking for a tool that was renamed in a later release
    /// narrows the agent with no error anywhere.
    pub fn requesting_tools(mut self, tools: &[String]) -> Self {
        self.requested_tools = tools.to_vec();
        self
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Set when the CLI reported a session id other than the one requested.
    pub fn session_mismatch(&self) -> Option<&str> {
        self.session_mismatch.as_deref()
    }

    /// Tools Zest asked for that the CLI did not resolve.
    pub fn dropped_tools(&self) -> &[String] {
        &self.dropped_tools
    }

    pub fn has_finish(&self) -> bool {
        self.has_finish
    }

    pub fn result_error(&self) -> Option<&str> {
        self.result_error.as_deref()
    }

    fn observe(&mut self, discriminant: &str) {
        self.ledger.observe(self.spec, discriminant);
    }

    /// Shorten an absolute path to something worth putting in a row title.
    fn display_path(&self, raw: &str) -> String {
        Path::new(raw)
            .strip_prefix(&self.root)
            .ok()
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
            .filter(|relative| !relative.is_empty())
            .unwrap_or_else(|| raw.to_string())
    }

    fn system(&mut self, raw: &Value) -> Vec<ExternalAgentEvent> {
        let subtype = raw.get("subtype").and_then(Value::as_str).unwrap_or("");
        self.observe(&format!("system:{subtype}"));
        if subtype != "init" {
            return Vec::new();
        }

        if let Some(session) = raw.get("session_id").and_then(Value::as_str) {
            self.session_id = Some(session.to_string());
            if let Some(expected) = &self.expected_session {
                if expected != session {
                    self.session_mismatch = Some(session.to_string());
                }
            }
        }

        if !self.requested_tools.is_empty() {
            if let Some(resolved) = raw.get("tools").and_then(Value::as_array) {
                let resolved: Vec<&str> = resolved.iter().filter_map(Value::as_str).collect();
                self.dropped_tools = self
                    .requested_tools
                    .iter()
                    .filter(|wanted| !resolved.iter().any(|got| got == wanted))
                    .cloned()
                    .collect();
            }
        }
        Vec::new()
    }

    /// Partial-message deltas: the only source of character-by-character text.
    fn stream_event(&mut self, raw: &Value) -> Vec<ExternalAgentEvent> {
        let event = raw.get("event").unwrap_or(raw);
        if event.get("type").and_then(Value::as_str) != Some("content_block_delta") {
            return Vec::new();
        }
        let Some(delta) = event.get("delta") else {
            return Vec::new();
        };

        // A subagent's deltas must never join the parent's answer: spliced in,
        // they corrupt the text the user reads and the text Zest persists.
        let from_subagent = subagent_parent(raw).is_some();

        match delta.get("type").and_then(Value::as_str) {
            Some("text_delta") => {
                self.observe("stream_event:text_delta");
                let Some(text) = delta.get("text").and_then(Value::as_str) else {
                    return Vec::new();
                };
                if from_subagent {
                    return Vec::new();
                }
                self.streamed_text.push_str(text);
                vec![ExternalAgentEvent::TextDelta(text.to_string())]
            }
            Some("thinking_delta") => {
                self.observe("stream_event:thinking_delta");
                let Some(text) = delta.get("thinking").and_then(Value::as_str) else {
                    return Vec::new();
                };
                if from_subagent {
                    return Vec::new();
                }
                vec![ExternalAgentEvent::Thinking(text.to_string())]
            }
            _ => Vec::new(),
        }
    }

    fn assistant(&mut self, raw: &Value) -> Vec<ExternalAgentEvent> {
        let Some(content) = raw
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
        else {
            return Vec::new();
        };

        // Text and thinking already reached the user as deltas. Re-emitting the
        // assembled block would print the whole answer a second time.
        if let Some(parent) = subagent_parent(raw) {
            return self.subagent_progress(parent, content);
        }

        let mut events = Vec::new();
        for block in content {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => self.observe("assistant:text"),
                Some("thinking") => self.observe("assistant:thinking"),
                Some("tool_use") => {
                    self.observe("assistant:tool_use");
                    if let Some(event) = self.tool_start(block) {
                        events.push(event);
                    }
                }
                Some(other) => self.observe(&format!("assistant:{other}")),
                None => {}
            }
        }
        events
    }

    /// A subagent said something. Show it on the parent `Task` row rather than
    /// in the transcript, which belongs to the parent agent.
    fn subagent_progress(&mut self, parent: &str, content: &[Value]) -> Vec<ExternalAgentEvent> {
        self.observe("assistant:subagent");
        let Some(pending) = self.pending.get(parent).cloned() else {
            return Vec::new();
        };
        let text = content
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" ");
        let summary = first_line(text.trim(), SUBAGENT_TITLE_CHARS);
        if summary.is_empty() {
            return Vec::new();
        }
        vec![ExternalAgentEvent::ToolCall {
            id: parent.to_string(),
            title: format!("{}: {summary}", pending.title),
            status: "in_progress".into(),
        }]
    }

    fn tool_start(&mut self, block: &Value) -> Option<ExternalAgentEvent> {
        let id = block.get("id").and_then(Value::as_str)?.to_string();
        let name = block
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_string();
        let input = block.get("input");
        let path = input
            .and_then(|input| string_field(input, &["file_path", "path", "notebook_path"]))
            .map(|raw| self.display_path(&raw));
        let title = self.tool_title(&name, input, path.as_deref());

        self.pending.insert(
            id.clone(),
            PendingTool {
                name: name.clone(),
                title: title.clone(),
                path: path.clone(),
            },
        );
        Some(ExternalAgentEvent::ToolCall {
            id,
            title,
            status: "in_progress".into(),
        })
    }

    /// What this tool call is about, in a few words.
    ///
    /// Ported from `inferClaudeToolStartSummary`, retargeted at the tool names
    /// CLI 2.1.220 actually ships: anygent's table still names `LS` and `Agent`,
    /// neither of which exists any more.
    fn tool_title(&self, name: &str, input: Option<&Value>, path: Option<&str>) -> String {
        let Some(input) = input else {
            return name.to_string();
        };
        let detail = match name {
            "Bash" | "PowerShell" => string_field(input, &["command"]),
            "Read" | "Write" | "Edit" | "NotebookEdit" => path.map(str::to_string),
            "Glob" => string_field(input, &["pattern"]),
            "Grep" => match (
                string_field(input, &["pattern"]),
                string_field(input, &["path"]),
            ) {
                (Some(pattern), Some(path)) => {
                    Some(format!("{pattern} in {}", self.display_path(&path)))
                }
                (Some(pattern), None) => Some(pattern),
                _ => None,
            },
            // The CLI calls this tool `Task` on `--tools` and `Agent` in the
            // `tool_use` block it emits. Both names reach here.
            "Task" | "Agent" => string_field(input, &["description", "subagent_type"]),
            "Skill" => string_field(input, &["skill", "command"]),
            "WebFetch" => string_field(input, &["url"]),
            "WebSearch" => string_field(input, &["query"]),
            "ToolSearch" => string_field(input, &["query"]),
            _ => string_field(input, &["command", "description", "query", "pattern"])
                .or_else(|| path.map(str::to_string)),
        };
        match detail {
            Some(detail) if !detail.trim().is_empty() => {
                format!("{name} {}", first_line(detail.trim(), 160))
            }
            _ => name.to_string(),
        }
    }

    fn user(&mut self, raw: &Value) -> Vec<ExternalAgentEvent> {
        let Some(content) = raw
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
        else {
            return Vec::new();
        };

        let mut events = Vec::new();
        for item in content {
            if item.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            self.observe("user:tool_result");
            let Some(id) = item.get("tool_use_id").and_then(Value::as_str) else {
                continue;
            };
            let is_error = item
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let pending = self.pending.remove(id);
            let title = pending
                .as_ref()
                .map(|pending| pending.title.clone())
                .unwrap_or_else(|| "tool".to_string());

            events.push(ExternalAgentEvent::ToolCall {
                id: id.to_string(),
                title,
                status: if is_error { "failed" } else { "completed" }.into(),
            });

            if is_error {
                continue;
            }
            // A completed edit is the only place the headless path can learn
            // that a file changed; there is no diff record in this stream.
            if let Some(pending) = pending {
                if EDIT_TOOLS.contains(&pending.name.as_str()) {
                    if let Some(path) = pending.path {
                        events.push(ExternalAgentEvent::Diff { path });
                    }
                }
            }
        }
        events
    }

    fn result(&mut self, raw: &Value) -> Vec<ExternalAgentEvent> {
        self.observe("result");
        self.has_finish = true;
        if let Some(session) = raw.get("session_id").and_then(Value::as_str) {
            self.session_id = Some(session.to_string());
        }

        let text = raw
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        if raw.get("is_error").and_then(Value::as_bool) == Some(true) {
            let message = if text.trim().is_empty() {
                "Claude Code returned an error result.".to_string()
            } else {
                text
            };
            self.result_error = Some(message.clone());
            return vec![ExternalAgentEvent::Error(message)];
        }

        // The answer already arrived as deltas in the ordinary case, and the
        // run assembles its text from those. Emitting the assembled copy too
        // would show the user every character twice.
        if text.trim().is_empty() || same_text(&text, &self.streamed_text) {
            return Vec::new();
        }
        vec![ExternalAgentEvent::Text(text)]
    }
}

impl StreamNormalizer for ClaudeNormalizer {
    fn normalize(&mut self, raw: &Value) -> Vec<ExternalAgentEvent> {
        self.ledger.line();
        match raw.get("type").and_then(Value::as_str) {
            Some("system") => self.system(raw),
            Some("stream_event") => self.stream_event(raw),
            Some("assistant") => self.assistant(raw),
            Some("user") => self.user(raw),
            Some("result") => self.result(raw),
            Some("rate_limit_event") => {
                // Parsed for quota by the runner; nothing to show.
                self.observe("rate_limit_event");
                Vec::new()
            }
            Some("error") => {
                self.observe("error");
                let message = raw
                    .get("error")
                    .and_then(error_text)
                    .or_else(|| error_text(raw))
                    .unwrap_or_else(|| "Claude Code reported an error.".to_string());
                self.result_error.get_or_insert(message.clone());
                vec![ExternalAgentEvent::Error(message)]
            }
            Some(other) => {
                self.observe(other);
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    fn parse_error(&mut self) {
        self.ledger.line();
        self.ledger.parse_error();
    }

    fn report(&self, outcome: RunOutcome) -> ContractReport {
        self.ledger.report(self.spec, outcome)
    }
}

/// The `Task` tool call a record belongs to, when it came from a subagent.
///
/// Only populated when Zest passes `--forward-subagent-text`; without it the
/// CLI does not emit subagent records at all and this is always `None`.
fn subagent_parent(raw: &Value) -> Option<&str> {
    raw.get("parent_tool_use_id").and_then(Value::as_str)
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|found| !found.is_empty())
            .map(str::to_string)
    })
}

fn error_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    string_field(value, &["message", "error", "detail"])
}

/// First line, clipped, for a row title.
fn first_line(value: &str, limit: usize) -> String {
    let line = value.lines().next().unwrap_or("").trim();
    if line.chars().count() <= limit {
        return line.to_string();
    }
    let clipped: String = line.chars().take(limit.saturating_sub(1)).collect();
    format!("{clipped}\u{2026}")
}

/// Whether two answers are the same text, ignoring whitespace.
fn same_text(left: &str, right: &str) -> bool {
    if left.trim().is_empty() || right.trim().is_empty() {
        return false;
    }
    left.split_whitespace().eq(right.split_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::stream_contract::ContractStatus;
    use serde_json::json;

    const ROOT: &str = if cfg!(windows) {
        r"D:\Proyectos IA\Zest-Harness"
    } else {
        "/work/zest"
    };

    fn absolute(relative: &str) -> String {
        if cfg!(windows) {
            format!(r"{ROOT}\{}", relative.replace('/', "\\"))
        } else {
            format!("{ROOT}/{relative}")
        }
    }

    fn normalizer() -> ClaudeNormalizer {
        ClaudeNormalizer::new(ROOT)
    }

    /// Captured from `claude 2.1.220`, trimmed to the fields that are read.
    fn init(session: &str) -> Value {
        json!({
            "type": "system",
            "subtype": "init",
            "session_id": session,
            "model": "claude-haiku-4-5-20251001",
            "tools": ["Bash", "Glob", "Grep", "Read", "WebFetch", "WebSearch"],
        })
    }

    fn tool_use(id: &str, name: &str, input: Value) -> Value {
        json!({
            "type": "assistant",
            "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": id, "name": name, "input": input}
            ]},
            "parent_tool_use_id": null,
        })
    }

    fn tool_result(id: &str, is_error: bool) -> Value {
        json!({
            "type": "user",
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": id, "is_error": is_error, "content": "ok"}
            ]},
            "parent_tool_use_id": null,
        })
    }

    fn delta(text: &str) -> Value {
        json!({
            "type": "stream_event",
            "parent_tool_use_id": null,
            "event": {
                "type": "content_block_delta",
                "delta": {"type": "text_delta", "text": text},
            },
        })
    }

    fn result(text: &str) -> Value {
        json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "session_id": "eb6e1162-1403-4fdd-a844-c2c71ab32a92",
            "result": text,
        })
    }

    /// The defect this module exists to fix: a completed tool call used to
    /// render as `External tool` because nothing correlated the result id back
    /// to the call that made it.
    #[test]
    fn a_completed_tool_call_keeps_the_name_it_started_with() {
        let mut normalizer = normalizer();
        let started = normalizer.normalize(&tool_use(
            "toolu_1",
            "Read",
            json!({"file_path": absolute("rust-toolchain.toml")}),
        ));
        assert_eq!(
            started,
            vec![ExternalAgentEvent::ToolCall {
                id: "toolu_1".into(),
                title: "Read rust-toolchain.toml".into(),
                status: "in_progress".into(),
            }]
        );

        let finished = normalizer.normalize(&tool_result("toolu_1", false));
        assert_eq!(
            finished,
            vec![ExternalAgentEvent::ToolCall {
                id: "toolu_1".into(),
                title: "Read rust-toolchain.toml".into(),
                status: "completed".into(),
            }]
        );
    }

    #[test]
    fn a_successful_edit_reports_the_file_it_changed() {
        let mut normalizer = normalizer();
        normalizer.normalize(&tool_use(
            "toolu_2",
            "Edit",
            json!({"file_path": absolute("crates/core/src/lib.rs")}),
        ));
        let events = normalizer.normalize(&tool_result("toolu_2", false));

        assert!(events.contains(&ExternalAgentEvent::Diff {
            path: "crates/core/src/lib.rs".into()
        }));
    }

    /// A failed edit changed nothing, so claiming a diff would be a lie.
    #[test]
    fn a_failed_edit_reports_no_diff() {
        let mut normalizer = normalizer();
        normalizer.normalize(&tool_use(
            "toolu_3",
            "Edit",
            json!({"file_path": absolute("src/lib.rs")}),
        ));
        let events = normalizer.normalize(&tool_result("toolu_3", true));

        assert!(!events
            .iter()
            .any(|event| matches!(event, ExternalAgentEvent::Diff { .. })));
        assert_eq!(
            events[0],
            ExternalAgentEvent::ToolCall {
                id: "toolu_3".into(),
                title: "Edit src/lib.rs".into(),
                status: "failed".into(),
            }
        );
    }

    #[test]
    fn command_tools_show_the_command() {
        let mut normalizer = normalizer();
        let events = normalizer.normalize(&tool_use(
            "toolu_4",
            "Bash",
            json!({"command": "cargo check"}),
        ));
        let ExternalAgentEvent::ToolCall { title, .. } = &events[0] else {
            panic!("expected a tool row: {events:?}");
        };
        assert_eq!(title, "Bash cargo check");
    }

    #[test]
    fn init_records_the_session_the_cli_chose() {
        let mut normalizer = normalizer().expecting_session("session-a");
        normalizer.normalize(&init("session-a"));

        assert_eq!(normalizer.session_id(), Some("session-a"));
        assert_eq!(normalizer.session_mismatch(), None);
    }

    /// Resuming into a different session silently would answer from the wrong
    /// conversation, so the disagreement has to be observable.
    #[test]
    fn a_session_the_cli_did_not_honour_is_flagged() {
        let mut normalizer = normalizer().expecting_session("session-a");
        normalizer.normalize(&init("session-b"));

        assert_eq!(normalizer.session_mismatch(), Some("session-b"));
    }

    /// `--tools` drops names it does not recognise without any error, so the
    /// only way to notice a renamed tool is to compare the two lists.
    #[test]
    fn tools_the_cli_silently_dropped_are_recorded() {
        let requested: Vec<String> = ["Read", "Bash", "LS"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut normalizer = normalizer().requesting_tools(&requested);
        normalizer.normalize(&init("session-a"));

        assert_eq!(normalizer.dropped_tools(), ["LS".to_string()]);
    }

    #[test]
    fn streamed_text_is_not_repeated_by_the_final_result() {
        let mut normalizer = normalizer();
        normalizer.normalize(&init("session-a"));
        normalizer.normalize(&delta("The channel is "));
        normalizer.normalize(&delta("`1.97.1`."));
        let events = normalizer.normalize(&result("The channel is `1.97.1`."));

        assert!(events.is_empty(), "{events:?}");
        assert!(normalizer.has_finish());
    }

    /// Without partial messages there are no deltas, so the result is the only
    /// copy of the answer and must be emitted.
    #[test]
    fn a_result_with_no_deltas_still_delivers_the_answer() {
        let mut normalizer = normalizer();
        normalizer.normalize(&init("session-a"));
        let events = normalizer.normalize(&result("Done."));

        assert_eq!(events, vec![ExternalAgentEvent::Text("Done.".into())]);
    }

    #[test]
    fn an_error_result_surfaces_the_reason() {
        let mut normalizer = normalizer();
        let mut error = result("model overloaded");
        error["is_error"] = json!(true);
        let events = normalizer.normalize(&error);

        assert_eq!(
            events,
            vec![ExternalAgentEvent::Error("model overloaded".into())]
        );
        assert_eq!(normalizer.result_error(), Some("model overloaded"));
    }

    /// A subagent's words are not the parent's answer. Splicing them into the
    /// delta stream would corrupt both the transcript and what is persisted.
    #[test]
    fn subagent_text_never_joins_the_parent_answer() {
        let mut normalizer = normalizer();
        // `Agent`, not `Task`: that is the name the CLI puts in the `tool_use`
        // block, even though `--tools` spells the same tool `Task`.
        normalizer.normalize(&tool_use(
            "toolu_task",
            "Agent",
            json!({"description": "audit the loader"}),
        ));

        let mut subagent = delta("internal chatter");
        subagent["parent_tool_use_id"] = json!("toolu_task");
        assert!(normalizer.normalize(&subagent).is_empty());

        normalizer.normalize(&delta("Final answer."));
        let events = normalizer.normalize(&result("Final answer."));
        assert!(events.is_empty(), "{events:?}");
    }

    #[test]
    fn subagent_messages_report_progress_on_the_task_row() {
        let mut normalizer = normalizer();
        // `Agent`, not `Task`: that is the name the CLI puts in the `tool_use`
        // block, even though `--tools` spells the same tool `Task`.
        normalizer.normalize(&tool_use(
            "toolu_task",
            "Agent",
            json!({"description": "audit the loader"}),
        ));

        let mut message = json!({
            "type": "assistant",
            "message": {"role": "assistant", "content": [{"type": "text", "text": "Reading the loader"}]},
        });
        message["parent_tool_use_id"] = json!("toolu_task");

        assert_eq!(
            normalizer.normalize(&message),
            vec![ExternalAgentEvent::ToolCall {
                id: "toolu_task".into(),
                title: "Agent audit the loader: Reading the loader".into(),
                status: "in_progress".into(),
            }]
        );
    }

    #[test]
    fn a_full_turn_satisfies_the_contract() {
        let mut normalizer = normalizer();
        normalizer.normalize(&init("session-a"));
        normalizer.normalize(&json!({"type": "system", "subtype": "thinking_tokens"}));
        normalizer.normalize(&tool_use(
            "toolu_1",
            "Read",
            json!({"file_path": absolute("a.txt")}),
        ));
        normalizer.normalize(&tool_result("toolu_1", false));
        normalizer.normalize(&delta("done"));
        normalizer.normalize(&result("done"));

        let report = normalizer.report(RunOutcome::Success);
        assert_eq!(report.status, ContractStatus::Ok, "{report:?}");
        assert_eq!(report.raw_lines, 6);
        assert_eq!(report.counts.get("system:init"), Some(&1));
        assert_eq!(report.counts.get("result"), Some(&1));
    }

    /// The signal that a future CLI release changed the stream underneath us.
    #[test]
    fn an_unrecognised_record_shows_up_as_drift() {
        let mut normalizer = normalizer();
        normalizer.normalize(&init("session-a"));
        normalizer.normalize(&json!({"type": "quantum_event"}));
        normalizer.normalize(&result("done"));

        let report = normalizer.report(RunOutcome::Success);
        assert_eq!(report.status, ContractStatus::Degraded);
        assert_eq!(report.unknown.get("quantum_event"), Some(&1));
    }

    #[test]
    fn a_turn_that_never_finished_violates_the_contract() {
        let mut normalizer = normalizer();
        normalizer.normalize(&init("session-a"));
        normalizer.parse_error();

        let report = normalizer.report(RunOutcome::Error);
        assert_eq!(report.status, ContractStatus::Violated);
        assert_eq!(report.parse_errors, 1);
        assert!(report.describe().unwrap().contains("missing: result"));
    }
}
