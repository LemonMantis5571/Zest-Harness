//! A thin local browser tool contract.
//!
//! The core crate owns the model-facing request shape and approval semantics;
//! the desktop crate supplies the actual local webview implementation. Keeping
//! that boundary here means the agent loop does not depend on Tauri (or on a
//! particular browser engine).

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::approval::{ApprovalPreview, ToolRisk};
use super::outcome::ToolOutcome;
use super::prepared::PreparedToolCall;
use super::Tool;

const MAX_URL_CHARS: usize = 4_096;
const MAX_LOCATOR_CHARS: usize = 512;
const MAX_TYPED_CHARS: usize = 20_000;
const MAX_SNAPSHOT_CHARS: usize = 20_000;
const MAX_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_TIMEOUT_MS: u64 = 8_000;

/// Operations supported by the local browser session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAction {
    /// Open or navigate the current browser session.
    Open,
    /// Return a bounded semantic snapshot of the current page.
    Snapshot,
    /// Click a semantic locator.
    Click,
    /// Replace the value of a text control and emit an input event.
    Type,
    /// Send a keyboard key to the located control, or to the active element.
    Press,
    /// Wait for a locator to appear, or for the page to become ready.
    Wait,
}

/// A small locator vocabulary shared by the model and local webview adapter.
///
/// CSS is available for precise cases. `role` + `name` / `text` covers the
/// common accessible controls without exposing coordinates or browser-engine
/// internals to the model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserLocator {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

impl BrowserLocator {
    fn validate(&self) -> Result<(), String> {
        if self.css.is_none() && self.role.is_none() && self.name.is_none() && self.text.is_none() {
            return Err("locator needs one of `css`, `role`, `name`, or `text`".into());
        }

        for (label, value) in [
            ("css", self.css.as_deref()),
            ("role", self.role.as_deref()),
            ("name", self.name.as_deref()),
            ("text", self.text.as_deref()),
        ] {
            if let Some(value) = value {
                if value.trim().is_empty() {
                    return Err(format!("locator field `{label}` cannot be empty"));
                }
                if value.chars().count() > MAX_LOCATOR_CHARS {
                    return Err(format!("locator field `{label}` is too long"));
                }
            }
        }

        Ok(())
    }

    /// Compact, non-secret label for an approval card.
    pub fn describe(&self) -> String {
        if let Some(css) = &self.css {
            return format!("css `{css}`");
        }
        let mut parts = Vec::new();
        if let Some(role) = &self.role {
            parts.push(format!("role `{role}`"));
        }
        if let Some(name) = &self.name {
            parts.push(format!("name `{name}`"));
        }
        if let Some(text) = &self.text {
            parts.push(format!("text `{text}`"));
        }
        let label = parts.join(", ");
        match self.index {
            Some(index) => format!("{label} at index {index}"),
            None => label,
        }
    }
}

/// Model-facing request sent to a [`BrowserAdapter`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserRequest {
    pub action: BrowserAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<BrowserLocator>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_chars: Option<usize>,
}

impl BrowserRequest {
    /// Parse and validate untrusted model JSON before it reaches the adapter.
    pub fn from_value(input: &Value) -> Result<Self, String> {
        let request: Self = serde_json::from_value(input.clone())
            .map_err(|error| format!("invalid browser request: {error}"))?;
        request.validate()?;
        Ok(request)
    }

    fn validate(&self) -> Result<(), String> {
        if let Some(url) = &self.url {
            if url.trim().is_empty() {
                return Err("browser `url` cannot be empty".into());
            }
            if url.chars().count() > MAX_URL_CHARS {
                return Err(format!(
                    "browser `url` is too long (max {MAX_URL_CHARS} chars)"
                ));
            }
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err("browser only opens http and https URLs".into());
            }
        }
        if let Some(text) = &self.text {
            if text.chars().count() > MAX_TYPED_CHARS {
                return Err(format!(
                    "browser `text` is too long (max {MAX_TYPED_CHARS} chars)"
                ));
            }
        }
        if let Some(key) = &self.key {
            if key.trim().is_empty() {
                return Err("browser `key` cannot be empty".into());
            }
            if key.chars().count() > 64 {
                return Err("browser `key` is too long".into());
            }
        }
        if let Some(timeout_ms) = self.timeout_ms {
            if timeout_ms == 0 || timeout_ms > MAX_TIMEOUT_MS {
                return Err(format!(
                    "browser `timeout_ms` must be between 1 and {MAX_TIMEOUT_MS}"
                ));
            }
        }
        if let Some(max_chars) = self.max_chars {
            if max_chars == 0 || max_chars > MAX_SNAPSHOT_CHARS {
                return Err(format!(
                    "browser `max_chars` must be between 1 and {MAX_SNAPSHOT_CHARS}"
                ));
            }
        }

        match self.action {
            BrowserAction::Open if self.url.is_none() => {
                Err("browser `open` requires `url`".into())
            }
            BrowserAction::Click if self.locator.is_none() => Err(format!(
                "browser `{}` requires `locator`",
                self.action_name()
            )),
            BrowserAction::Type if self.locator.is_none() => {
                Err("browser `type` requires `locator`".into())
            }
            BrowserAction::Type if self.text.is_none() => {
                Err("browser `type` requires `text`".into())
            }
            BrowserAction::Press if self.key.is_none() => {
                Err("browser `press` requires `key`".into())
            }
            _ => {
                if let Some(locator) = &self.locator {
                    locator.validate()?;
                }
                Ok(())
            }
        }
    }

    fn action_name(&self) -> &'static str {
        match self.action {
            BrowserAction::Open => "open",
            BrowserAction::Snapshot => "snapshot",
            BrowserAction::Click => "click",
            BrowserAction::Type => "type",
            BrowserAction::Press => "press",
            BrowserAction::Wait => "wait",
        }
    }

    fn risk(&self) -> ToolRisk {
        match self.action {
            BrowserAction::Click | BrowserAction::Type | BrowserAction::Press => ToolRisk::Exec,
            // A snapshot can contain an authenticated page, so keep the live
            // result available to the model but redact it from persisted wire
            // history using the existing sensitive-read path.
            BrowserAction::Snapshot => ToolRisk::Sensitive,
            BrowserAction::Open | BrowserAction::Wait => ToolRisk::Read,
        }
    }

    fn preview(&self) -> ApprovalPreview {
        let path = self
            .url
            .clone()
            .or_else(|| self.locator.as_ref().map(BrowserLocator::describe))
            .unwrap_or_else(|| "current browser page".into());
        let summary = match self.action {
            BrowserAction::Open => format!("open {path}"),
            BrowserAction::Snapshot => "read the current browser page".into(),
            BrowserAction::Click => format!("click {path}"),
            BrowserAction::Type => format!("type into {path}"),
            BrowserAction::Press => format!("press `{}`", self.key.as_deref().unwrap_or_default()),
            BrowserAction::Wait => format!("wait for {path}"),
        };
        ApprovalPreview {
            path,
            summary,
            diff: String::new(),
        }
    }

    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS)
    }

    pub fn snapshot_limit(&self) -> usize {
        self.max_chars.unwrap_or(12_000)
    }
}

/// The desktop's local webview implementation.
#[async_trait]
pub trait BrowserAdapter: Send + Sync {
    async fn execute(&self, request: BrowserRequest) -> Result<Value, String>;
}

/// Model-facing tool that delegates browser work to the local adapter.
pub struct BrowserTool {
    adapter: Arc<dyn BrowserAdapter>,
}

impl BrowserTool {
    pub fn new(adapter: Arc<dyn BrowserAdapter>) -> Self {
        Self { adapter }
    }
}

impl std::fmt::Debug for BrowserTool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BrowserTool")
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "Use Zest's local browser session. Open pages, inspect a bounded semantic snapshot, and interact with controls using a CSS or accessible locator. Snapshots may contain private page content and are treated as sensitive reads; browser clicks, typing, and key presses require permission."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["open", "snapshot", "click", "type", "press", "wait"]
                },
                "url": {
                    "type": "string",
                    "description": "HTTP or HTTPS URL for open"
                },
                "locator": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "css": { "type": "string" },
                        "role": { "type": "string" },
                        "name": { "type": "string" },
                        "text": { "type": "string" },
                        "index": { "type": "integer", "minimum": 0 }
                    },
                    "description": "Prefer role/name or text; use css for precise selectors"
                },
                "text": {
                    "type": "string",
                    "description": "Text to replace in a located input or textarea"
                },
                "key": {
                    "type": "string",
                    "description": "Keyboard key such as Enter, Tab, Escape, or ArrowDown"
                },
                "timeout_ms": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_TIMEOUT_MS
                },
                "max_chars": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_SNAPSHOT_CHARS
                }
            },
            "required": ["action"]
        })
    }

    fn risk(&self) -> ToolRisk {
        // The exact request risk is selected in `prepare`; this conservative
        // value is used only by callers that inspect the registry before a
        // request exists.
        ToolRisk::Exec
    }

    fn prepare(&self, input: Value) -> Result<PreparedToolCall, String> {
        let request = BrowserRequest::from_value(&input)?;
        Ok(PreparedToolCall::plain_with_preview(
            self.name(),
            request.risk(),
            input,
            request.preview(),
        ))
    }

    async fn run(&self, input: Value) -> Result<ToolOutcome, String> {
        let request = BrowserRequest::from_value(&input)?;
        let result = self.adapter.execute(request).await?;
        let body = match result {
            Value::String(text) => text,
            value => serde_json::to_string_pretty(&value)
                .map_err(|error| format!("serialize browser result: {error}"))?,
        };
        Ok(ToolOutcome::text(body))
    }
}

/// Register the parent-only browser tool. It intentionally is not part of the
/// delegated worker registry because a worker must not control the user's UI
/// session or inherit its cookies.
pub fn register_browser_tool(registry: &mut super::ToolRegistry, adapter: Arc<dyn BrowserAdapter>) {
    registry.register(Arc::new(BrowserTool::new(adapter)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct StubBrowser {
        seen: Mutex<Vec<BrowserRequest>>,
    }

    #[async_trait]
    impl BrowserAdapter for StubBrowser {
        async fn execute(&self, request: BrowserRequest) -> Result<Value, String> {
            self.seen.lock().unwrap().push(request);
            Ok(json!({ "ok": true, "title": "fixture" }))
        }
    }

    #[test]
    fn validates_action_specific_inputs() {
        let error = BrowserRequest::from_value(&json!({ "action": "click" })).unwrap_err();
        assert!(error.contains("requires `locator`"), "{error}");

        let error = BrowserRequest::from_value(&json!({
            "action": "open",
            "url": "file:///secret.txt"
        }))
        .unwrap_err();
        assert!(error.contains("only opens http and https"), "{error}");

        let wait = BrowserRequest::from_value(&json!({ "action": "wait" })).unwrap();
        assert_eq!(wait.action, BrowserAction::Wait);
    }

    #[test]
    fn interaction_requests_are_gated_and_do_not_preview_typed_text() {
        let tool = BrowserTool::new(Arc::new(StubBrowser {
            seen: Mutex::new(Vec::new()),
        }));
        let prepared = tool
            .prepare(json!({
                "action": "type",
                "locator": { "role": "textbox", "name": "Email" },
                "text": "a-secret-value"
            }))
            .unwrap();
        assert_eq!(prepared.risk, ToolRisk::Exec);
        assert!(prepared.preview.summary.contains("Email"));
        assert!(!prepared.preview.summary.contains("a-secret-value"));

        let snapshot = tool
            .prepare(serde_json::json!({ "action": "snapshot" }))
            .unwrap();
        assert_eq!(snapshot.risk, ToolRisk::Sensitive);
    }

    #[tokio::test]
    async fn tool_returns_adapter_json_and_preserves_request() {
        let adapter = Arc::new(StubBrowser {
            seen: Mutex::new(Vec::new()),
        });
        let tool = BrowserTool::new(adapter.clone());
        let outcome = tool
            .run(json!({ "action": "snapshot", "max_chars": 4000 }))
            .await
            .unwrap();
        assert!(outcome.body.contains("fixture"));
        assert_eq!(adapter.seen.lock().unwrap().len(), 1);
    }
}
