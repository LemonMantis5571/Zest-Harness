//! Claude Code's `can_use_tool` control channel.
//!
//! The CLI asks permission over its own stdout/stdin control protocol when it is
//! started with `--permission-prompt-tool stdio`. Without that flag it decides
//! locally and simply denies whatever it cannot auto-approve, which is why a
//! driver that only sets `--input-format stream-json` never sees a request.
//! `--input-format stream-json` also means the prompt is a JSON user message on
//! stdin, not an argv leftover — and an inbound `initialize` control request
//! has to be acknowledged or the CLI sits idle.
//!
//! This module is the translation layer and nothing else: request in, decision
//! out. Process lifetime lives in [`super::session::JsonlProcess`] and the turn
//! loop in [`super::claude_code`].
//!
//! # Failing closed
//!
//! Every path that cannot reach a human answers `deny`. A missing host, a
//! request without an id, a tool nobody mapped — all deny, because the
//! alternative is a coding agent editing a repository with nobody watching. That
//! is deliberately the opposite of the ACP permission path, which auto-selects
//! the first allow option when no host is attached.

use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};

use super::{ProviderCommandRequest, ProviderFileChangeRequest, ProviderInteractionHost};
use crate::tools::approval::ToolRisk;
use crate::tools::project::ProjectRoot;
use crate::tools::write_file::bounded_unified_diff;

/// Cap on a rendered diff handed to the approval card.
const MAX_DIFF_BYTES: usize = 64 * 1024;

/// What the CLI is asking to do, in zest's vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolPermissionRequest {
    /// Correlates the response. The CLI rejects an answer that omits it.
    pub request_id: String,
    pub tool_name: String,
    /// The CLI's own short label, typically a file name.
    pub description: Option<String>,
    pub input: Value,
}

impl ToolPermissionRequest {
    /// Read a `can_use_tool` control request. `None` when this is not one.
    pub(crate) fn parse(message: &Value) -> Option<Self> {
        if message.get("type").and_then(Value::as_str) != Some("control_request") {
            return None;
        }
        let request = message.get("request")?;
        if request.get("subtype").and_then(Value::as_str) != Some("can_use_tool") {
            return None;
        }
        // A request with no id is unanswerable: the CLI correlates purely on it,
        // so inventing one would resolve some other prompt.
        let request_id = message.get("request_id")?.as_str()?.to_string();
        Some(Self {
            request_id,
            tool_name: request
                .get("tool_name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            description: request
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string),
            input: request.get("input").cloned().unwrap_or(Value::Null),
        })
    }

    fn field(&self, key: &str) -> Option<&str> {
        self.input.get(key).and_then(Value::as_str)
    }
}

/// Which approval surface a tool belongs on.
///
/// Keyed on behaviour rather than on an allow-list: an unrecognised name — a new
/// built-in, or any `mcp__*` tool from a server zest knows nothing about — lands
/// on the command surface and is asked about, never permitted by omission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Surface {
    Command(ToolRisk),
    FileChange,
}

pub(crate) fn surface_for(tool_name: &str) -> Surface {
    match tool_name {
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => Surface::FileChange,
        "Bash" | "BashOutput" | "KillShell" => Surface::Command(ToolRisk::Exec),
        "Read" | "Glob" | "Grep" | "WebFetch" | "WebSearch" => Surface::Command(ToolRisk::Read),
        _ => Surface::Command(ToolRisk::Exec),
    }
}

/// The JSON user message `--input-format stream-json` waits for on stdin.
///
/// Observed shape from Claude Code 2.1.x hosts: a `user` envelope, not the
/// prompt as a raw argv string. `session_id` is empty in stdio / `--print` mode.
pub(crate) fn stream_json_user_message(prompt: &str) -> Value {
    json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": prompt,
        },
        "parent_tool_use_id": Value::Null,
        "session_id": "",
    })
}

/// An inbound `initialize` handshake. `None` when this is not one, or when it
/// has no id to correlate a reply on.
pub(crate) fn initialize_request_id(message: &Value) -> Option<&str> {
    if message.get("type").and_then(Value::as_str) != Some("control_request") {
        return None;
    }
    let request = message.get("request")?;
    if request.get("subtype").and_then(Value::as_str) != Some("initialize") {
        return None;
    }
    message.get("request_id").and_then(Value::as_str)
}

/// Acknowledge an inbound initialize so the CLI continues the turn.
pub(crate) fn initialize_response(request_id: &str) -> Value {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": {},
        },
    })
}

/// The decision, in the shape the CLI's schema accepts.
///
/// `message` is mandatory on a deny — the CLI rejects a `deny` without one — and
/// it reaches the model verbatim as the tool result, so it is worth writing for
/// the model rather than for a human reading a dialog.
pub(crate) fn control_response(request_id: &str, allowed: bool, deny_reason: &str) -> Value {
    let decision = if allowed {
        json!({ "behavior": "allow" })
    } else {
        json!({ "behavior": "deny", "message": deny_reason })
    };
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": decision,
        },
    })
}

/// A one-line summary for the approval card.
pub(crate) fn summarize(request: &ToolPermissionRequest) -> String {
    let detail = match request.tool_name.as_str() {
        "Bash" => request.field("command").map(str::to_string),
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => request
            .field("file_path")
            .or_else(|| request.field("notebook_path"))
            .map(str::to_string),
        "Grep" => request.field("pattern").map(|p| format!("pattern {p}")),
        "Glob" => request.field("pattern").map(str::to_string),
        "WebFetch" => request.field("url").map(str::to_string),
        _ => None,
    };
    let detail = detail
        .or_else(|| request.description.clone())
        .unwrap_or_default();
    if detail.is_empty() {
        format!("Claude Code requested {}", request.tool_name)
    } else {
        format!("{}: {detail}", request.tool_name)
    }
}

/// Render the change a write-class tool is proposing, as a unified diff.
///
/// The CLI sends no diff, so zest builds one against the file on disk. The path
/// is resolved through [`ProjectRoot`] first: it arrives as model output, and a
/// request naming something outside the project is reported on the card rather
/// than read anyway to render a nicer diff.
pub(crate) fn render_diff(root: &Path, request: &ToolPermissionRequest) -> (String, String) {
    let raw = request
        .field("file_path")
        .or_else(|| request.field("notebook_path"))
        .unwrap_or_default();
    let Ok(project) = ProjectRoot::new(root) else {
        return (raw.to_string(), String::new());
    };
    let Ok(resolved) = project.resolve(raw) else {
        return (
            raw.to_string(),
            format!("(no diff: {raw} is not inside this project)\n"),
        );
    };
    let relative = project.relativize(&resolved);
    let existed = resolved.is_file();
    let old = if existed {
        std::fs::read_to_string(&resolved).unwrap_or_default()
    } else {
        String::new()
    };

    let new = match request.tool_name.as_str() {
        "Write" => request.field("content").unwrap_or_default().to_string(),
        "Edit" => match (request.field("old_string"), request.field("new_string")) {
            (Some(from), Some(to)) => {
                let replace_all = request
                    .input
                    .get("replace_all")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if replace_all {
                    old.replace(from, to)
                } else {
                    old.replacen(from, to, 1)
                }
            }
            _ => return (relative, String::new()),
        },
        // MultiEdit and notebook edits carry shapes this renderer does not
        // reconstruct. The card still names the path and the tool; it simply
        // does not claim to show the change.
        _ => return (relative, String::new()),
    };

    let mut diff = bounded_unified_diff(&relative, &old, &new, existed);
    if diff.len() > MAX_DIFF_BYTES {
        diff = crate::bounded::ends_within(&diff, MAX_DIFF_BYTES, |omitted| {
            format!("\n[... {omitted} bytes of diff omitted ...]\n")
        })
        .unwrap_or(diff);
    }
    (relative, diff)
}

/// Ask the front-end, or deny when there is nobody to ask.
pub(crate) async fn decide(
    host: Option<&Arc<dyn ProviderInteractionHost>>,
    approval_id: &str,
    surface: Surface,
    path: String,
    summary: String,
    diff: String,
) -> bool {
    let Some(host) = host else {
        return false;
    };
    match surface {
        Surface::FileChange => {
            host.prepare_file_change_approval(approval_id).await;
            host.approve_file_change(ProviderFileChangeRequest {
                approval_id: approval_id.to_string(),
                path: Some(path),
                diff: (!diff.is_empty()).then_some(diff),
                reason: Some(summary),
            })
            .await
        }
        Surface::Command(_) => {
            host.prepare_command_approval(approval_id).await;
            host.approve_command(ProviderCommandRequest {
                approval_id: approval_id.to_string(),
                command: summary,
                cwd: (!path.is_empty()).then_some(path),
                reason: None,
            })
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(tool: &str, input: Value) -> ToolPermissionRequest {
        ToolPermissionRequest {
            request_id: "req-1".into(),
            tool_name: tool.into(),
            description: None,
            input,
        }
    }

    /// The exact envelope observed from claude 2.1.220.
    #[test]
    fn a_can_use_tool_request_is_parsed_from_the_observed_envelope() {
        let message = json!({
            "type": "control_request",
            "request_id": "8fbabe69-0828-416f-9688-a4a9c10079aa",
            "request": {
                "subtype": "can_use_tool",
                "tool_name": "Edit",
                "display_name": "Edit",
                "description": "notes.txt",
                "tool_use_id": "toolu_01F2",
                "input": { "file_path": "notes.txt", "old_string": "a", "new_string": "b" },
            },
        });
        let parsed = ToolPermissionRequest::parse(&message).expect("a can_use_tool request");
        assert_eq!(parsed.request_id, "8fbabe69-0828-416f-9688-a4a9c10079aa");
        assert_eq!(parsed.tool_name, "Edit");
        assert_eq!(parsed.description.as_deref(), Some("notes.txt"));
    }

    #[test]
    fn anything_that_is_not_a_can_use_tool_request_is_ignored() {
        for other in [
            json!({ "type": "assistant", "message": {} }),
            json!({ "type": "control_request", "request_id": "x",
                    "request": { "subtype": "initialize" } }),
            // No id to correlate on: unanswerable, so not a request we accept.
            json!({ "type": "control_request",
                    "request": { "subtype": "can_use_tool", "tool_name": "Edit" } }),
        ] {
            assert!(ToolPermissionRequest::parse(&other).is_none(), "{other}");
        }
    }

    #[test]
    fn an_initialize_request_with_an_id_is_acknowledged() {
        let message = json!({
            "type": "control_request",
            "request_id": "init-1",
            "request": { "subtype": "initialize" },
        });
        assert_eq!(initialize_request_id(&message), Some("init-1"));
        let reply = initialize_response("init-1");
        assert_eq!(reply["type"], "control_response");
        assert_eq!(reply["response"]["request_id"], "init-1");
        assert_eq!(reply["response"]["subtype"], "success");
    }

    #[test]
    fn initialize_without_an_id_is_unanswerable() {
        let message = json!({
            "type": "control_request",
            "request": { "subtype": "initialize" },
        });
        assert!(initialize_request_id(&message).is_none());
    }

    #[test]
    fn stream_json_user_message_is_a_user_envelope() {
        let message = stream_json_user_message("inspect the loader");
        assert_eq!(message["type"], "user");
        assert_eq!(message["message"]["role"], "user");
        assert_eq!(message["message"]["content"], "inspect the loader");
    }

    #[test]
    fn an_unknown_tool_falls_back_to_asking_rather_than_allowing() {
        for unknown in ["mcp__github__create_pr", "SomeFutureTool", ""] {
            assert_eq!(
                surface_for(unknown),
                Surface::Command(ToolRisk::Exec),
                "{unknown} should still be asked about"
            );
        }
        assert_eq!(surface_for("Edit"), Surface::FileChange);
        assert_eq!(surface_for("Bash"), Surface::Command(ToolRisk::Exec));
        assert_eq!(surface_for("Read"), Surface::Command(ToolRisk::Read));
    }

    #[test]
    fn a_deny_always_carries_the_message_the_cli_requires() {
        let denied = control_response("req-1", false, "the user declined this edit");
        let decision = &denied["response"]["response"];
        assert_eq!(decision["behavior"], "deny");
        assert_eq!(decision["message"], "the user declined this edit");
        assert_eq!(denied["response"]["request_id"], "req-1");
        assert_eq!(denied["response"]["subtype"], "success");

        // An allow carries no message, and must not invent one.
        let allowed = control_response("req-1", true, "unused");
        assert_eq!(allowed["response"]["response"]["behavior"], "allow");
        assert!(allowed["response"]["response"].get("message").is_none());
    }

    #[test]
    fn an_edit_renders_a_diff_against_the_file_on_disk() {
        let dir = std::env::temp_dir().join(format!("zest-cc-diff-{}", crate::thread::new_id("t")));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), "first line\n").unwrap();

        let (path, diff) = render_diff(
            &dir,
            &request(
                "Edit",
                json!({ "file_path": "notes.txt", "old_string": "first line",
                        "new_string": "first line\nhello" }),
            ),
        );
        assert_eq!(path, "notes.txt");
        assert!(diff.contains("+hello"), "{diff}");
        assert!(diff.contains("--- a/notes.txt"), "{diff}");
    }

    #[test]
    fn a_path_outside_the_project_is_reported_instead_of_read() {
        let dir = std::env::temp_dir().join(format!("zest-cc-esc-{}", crate::thread::new_id("t")));
        std::fs::create_dir_all(&dir).unwrap();

        let (_, diff) = render_diff(
            &dir,
            &request(
                "Write",
                json!({ "file_path": "../../../../etc/passwd", "content": "x" }),
            ),
        );
        assert!(diff.contains("not inside this project"), "{diff}");
        assert!(!diff.contains("root:"), "the file must not have been read");
    }

    #[test]
    fn a_summary_names_the_tool_and_its_target() {
        assert_eq!(
            summarize(&request("Bash", json!({ "command": "cargo test" }))),
            "Bash: cargo test"
        );
        assert_eq!(
            summarize(&request("Edit", json!({ "file_path": "src/main.rs" }))),
            "Edit: src/main.rs"
        );
        // Nothing recognisable still produces something a card can render.
        assert_eq!(
            summarize(&request("mcp__x__y", json!({}))),
            "Claude Code requested mcp__x__y"
        );
    }

    #[tokio::test]
    async fn with_no_host_every_request_is_denied_without_awaiting_anything() {
        for surface in [
            Surface::FileChange,
            Surface::Command(ToolRisk::Exec),
            Surface::Command(ToolRisk::Read),
        ] {
            let allowed = decide(
                None,
                "approval-1",
                surface,
                "notes.txt".into(),
                "Edit: notes.txt".into(),
                String::new(),
            )
            .await;
            assert!(
                !allowed,
                "a headless turn must not self-approve {surface:?}"
            );
        }
    }
}
