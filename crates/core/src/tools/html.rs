//! Parent-only publication and gated screenshot feedback for interactive documents.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::approval::ToolRisk;
use super::outcome::ToolImage;
use super::prepared::PreparedToolCall;
use super::{Tool, ToolMetadata, ToolOutcome, ToolRegistry};
use crate::html::HtmlDocument;
use crate::html_preview::HtmlPreviewer;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    title: String,
    html: String,
}

fn document(input: Value) -> Result<HtmlDocument, String> {
    let input: Input =
        serde_json::from_value(input).map_err(|e| format!("invalid HTML input: {e}"))?;
    HtmlDocument::parse(input.title, input.html)
}

pub fn register_html_tools(registry: &mut ToolRegistry, preview: bool) {
    if preview {
        registry.register(std::sync::Arc::new(HtmlTool { preview: true }));
    }
    registry.register(std::sync::Arc::new(HtmlTool { preview: false }));
}

struct HtmlTool {
    preview: bool,
}

#[async_trait]
impl Tool for HtmlTool {
    fn name(&self) -> &str {
        if self.preview {
            "html_preview"
        } else {
            "html_render"
        }
    }
    fn description(&self) -> &str {
        if self.preview {
            "Preview self-contained HTML in a disposable offline sandbox. Returns a real PNG screenshot and bounded console/errors. Requires execution approval and an installed Chromium/Edge. Inline scripts/styles and embedded images only."
        } else {
            "Publish a self-contained interactive HTML document as an inert chat card. The user opens it in an offline sandbox. No external assets/network or local files. Use html_preview first when available."
        }
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"title":{"type":"string","description":"Short descriptive title (1–120 characters)"},"html":{"type":"string","description":"Complete self-contained HTML, max 512 KiB. Inline CSS/JS and data images only."}},"required":["title","html"],"additionalProperties":false})
    }
    fn risk(&self) -> ToolRisk {
        if self.preview {
            ToolRisk::Exec
        } else {
            ToolRisk::Read
        }
    }
    fn prepare(&self, input: Value) -> Result<PreparedToolCall, String> {
        let doc = document(input.clone())?;
        let mut prepared = PreparedToolCall::plain(self.name(), self.risk(), input);
        if self.preview {
            let mut identity = blake3::Hasher::new();
            identity.update(doc.title().as_bytes());
            identity.update(&[0]);
            identity.update(doc.source().as_bytes());
            prepared.preview.path = format!("html-document:{}", identity.finalize().to_hex());
        }
        prepared.preview.summary = format!(
            "{}: {} ({} bytes; offline)",
            self.name(),
            doc.title(),
            doc.source().len()
        );
        Ok(prepared)
    }
    async fn run(&self, input: Value) -> Result<ToolOutcome, String> {
        let doc = document(input)?;
        if self.preview {
            let report = HtmlPreviewer::installed().preview(&doc).await?;
            let mut outcome = ToolOutcome::text(format!(
                "Captured a 1024×768 PNG. Console/errors/blocked requests:\n{}",
                if report.diagnostics.is_empty() {
                    "None observed".into()
                } else {
                    report.diagnostics.join("\n")
                }
            ));
            outcome.images.push(ToolImage::png(report.png_base64)?);
            Ok(outcome)
        } else {
            Ok(ToolOutcome::with_metadata(format!("Published {} in chat. The user can Open, expand, inspect source, reset, or save it.", doc.title()), ToolMetadata::HtmlDocument { title: doc.title().into(), html: doc.source().into() }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::approval::{ApprovalMode, ApprovalPolicy, PolicyOutcome};

    #[test]
    fn session_preview_approval_is_bound_to_the_exact_document() {
        let preview = HtmlTool { preview: true };
        let first = json!({"title":"Counter","html":"<script>window.value=1</script>"});
        let prepare = |input| preview.prepare(input).unwrap();
        let approved = prepare(first.clone());
        for mode in [
            ApprovalMode::Manual,
            ApprovalMode::AcceptEdits,
            ApprovalMode::Auto,
        ] {
            let mut policy = ApprovalPolicy::new(mode);
            let decide = |policy: &ApprovalPolicy, call: &PreparedToolCall| {
                policy.decide(
                    &call.tool_name,
                    &call.preview.path,
                    call.risk,
                    call.auto_eligible,
                )
            };
            assert_eq!(decide(&policy, &approved), PolicyOutcome::Ask);
            policy.trust(&approved.tool_name, &approved.preview.path);
            assert_eq!(
                decide(&policy, &prepare(first.clone())),
                PolicyOutcome::Allow
            );
            assert_eq!(
                decide(
                    &policy,
                    &prepare(json!({"title":"Counter","html":"<script>window.value=2</script>"}))
                ),
                PolicyOutcome::Ask
            );
            assert_eq!(
                decide(
                    &policy,
                    &prepare(
                        json!({"title":"Different counter","html":"<script>window.value=1</script>"})
                    )
                ),
                PolicyOutcome::Ask
            );
        }
    }

    #[tokio::test]
    async fn render_is_inert_and_preview_requires_execution_approval() {
        let render = HtmlTool { preview: false };
        let outcome = render
            .run(json!({"title":"Counter","html":"<script>throw Error('not run')</script>"}))
            .await
            .unwrap();
        assert!(matches!(
            outcome.metadata,
            Some(ToolMetadata::HtmlDocument { .. })
        ));
        assert!(outcome.images.is_empty());
        let preview = HtmlTool { preview: true };
        let prepared = preview
            .prepare(json!({"title":"Counter","html":"<button>1</button>"}))
            .unwrap();
        assert_eq!(prepared.risk, ToolRisk::Exec);
        assert!(!prepared.auto_eligible);
        assert!(render
            .prepare(json!({"title":"Counter","html":""}))
            .is_err());
    }
}
