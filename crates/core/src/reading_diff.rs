//! Model-assisted, display-only reading diffs.
//!
//! The model returns coordinates into the original unified diff. Zest applies
//! those coordinates locally, so the model never authors an applicable patch.
//! Callers must keep the original diff available as the safety source of truth.

use std::sync::Arc;

use crate::anthropic::types::{text_of, Message};
use crate::error::{HarnessError, Result};
use crate::provider::{Provider, StreamEvent, TurnRequest};
use serde::{Deserialize, Serialize};

const SYSTEM: &str = "You produce conservative, display-only reading diffs for code review. Never invent code. Return JSON only.";

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LineRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReadingDiffPlan {
    #[serde(default)]
    pub remove: Vec<LineRange>,
    #[serde(default)]
    pub fold: Vec<LineRange>,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadingDiffResult {
    pub diff: String,
    pub summary: String,
    pub removed_lines: usize,
    pub folded_lines: usize,
}

/// Ask the active provider for a conservative compression plan and apply it.
pub async fn abridge(
    provider: Arc<dyn Provider>,
    model: &str,
    effort: &str,
    unified_diff: &str,
) -> Result<ReadingDiffResult> {
    if unified_diff.trim().is_empty() {
        return Ok(ReadingDiffResult {
            diff: String::new(),
            summary: "No changes.".into(),
            removed_lines: 0,
            folded_lines: 0,
        });
    }

    let numbered = unified_diff
        .lines()
        .enumerate()
        .map(|(i, line)| format!("{}|{}", i + 1, line))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!(
        "Abridge this unified diff for a human reviewer. Remove only obvious mechanical noise: imports, generated-code sections, repetitive unchanged context, and forced plumbing. Fold repetitive hunk source lines when their existence matters. Keep uncertain or behavior-bearing lines. Coordinates are 1-based physical lines in the ORIGINAL diff. Only choose lines inside hunks for remove/fold; never remove metadata or hunk headers. Return exactly this JSON shape: {{\"remove\":[{{\"start\":1,\"end\":2}}],\"fold\":[{{\"start\":3,\"end\":6}}],\"summary\":\"one sentence\"}}. Fold ranges must contain at least two contiguous lines with the same diff marker.\n\nORIGINAL DIFF:\n```diff\n{}\n```",
        numbered
    );
    let request = TurnRequest {
        model: model.to_string(),
        system: Some(SYSTEM.into()),
        messages: vec![Message::user_text(prompt)],
        tools: Vec::new(),
        allow_tool_use: false,
        max_tokens: 4_000,
        effort: Some(effort.to_string()),
        thinking: false,
        provider_session: None,
        interaction: None,
        cancel: None,
    };
    let mut sink = |_event: StreamEvent<'_>| {};
    let completion = provider.stream_turn(&request, &mut sink).await?;
    let text = text_of(&completion.content);
    let plan = parse_plan(&text)?;
    apply_plan(unified_diff, &plan)
}

fn parse_plan(text: &str) -> Result<ReadingDiffPlan> {
    let candidate = text
        .trim()
        .strip_prefix("```json")
        .or_else(|| text.trim().strip_prefix("```"))
        .unwrap_or(text.trim())
        .trim_end_matches('`')
        .trim();
    serde_json::from_str(candidate).map_err(|e| {
        HarnessError::Other(format!("reading diff provider returned invalid JSON: {e}"))
    })
}

fn is_hunk_source(line: &str) -> bool {
    line.starts_with('+') && !line.starts_with("+++")
        || line.starts_with('-') && !line.starts_with("---")
        || line.starts_with(' ')
}

fn validate_range(range: LineRange, lines: &[&str], allow_single: bool) -> Result<()> {
    if range.start == 0 || range.start > range.end || range.end > lines.len() {
        return Err(HarnessError::Other(format!(
            "reading diff range {}..{} is outside the original diff",
            range.start, range.end
        )));
    }
    if !allow_single && range.start == range.end {
        return Err(HarnessError::Other(
            "reading diff fold range must contain at least two lines".into(),
        ));
    }
    if !lines[range.start - 1..range.end]
        .iter()
        .all(|line| is_hunk_source(line))
    {
        return Err(HarnessError::Other(
            "reading diff plan targeted metadata instead of hunk source".into(),
        ));
    }
    Ok(())
}

fn apply_plan(unified_diff: &str, plan: &ReadingDiffPlan) -> Result<ReadingDiffResult> {
    let lines = unified_diff.lines().collect::<Vec<_>>();
    for range in &plan.remove {
        validate_range(*range, &lines, true)?;
    }
    for range in &plan.fold {
        validate_range(*range, &lines, false)?;
        let marker = lines[range.start - 1].chars().next();
        if !lines[range.start - 1..range.end]
            .iter()
            .all(|line| line.chars().next() == marker)
        {
            return Err(HarnessError::Other(
                "reading diff fold range crosses diff markers".into(),
            ));
        }
    }

    let removed = plan
        .remove
        .iter()
        .map(|r| r.end - r.start + 1)
        .sum::<usize>();
    let folded = plan.fold.iter().map(|r| r.end - r.start).sum::<usize>();
    let mut output = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let line_no = index + 1;
        if plan
            .remove
            .iter()
            .any(|r| r.start <= line_no && line_no <= r.end)
        {
            continue;
        }
        if let Some(range) = plan.fold.iter().find(|r| r.start == line_no) {
            let marker = line.chars().next().unwrap_or(' ');
            output.push(format!("{}...", marker));
            if range.end > line_no {
                continue;
            }
        }
        if plan
            .fold
            .iter()
            .any(|r| r.start < line_no && line_no <= r.end)
        {
            continue;
        }
        output.push((*line).to_string());
    }
    Ok(ReadingDiffResult {
        diff: output.join("\n"),
        summary: if plan.summary.trim().is_empty() {
            "Reading diff generated.".into()
        } else {
            plan.summary.trim().to_string()
        },
        removed_lines: removed,
        folded_lines: folded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_only_display_plan_and_preserves_metadata() {
        let diff = "diff --git a/a.rs b/a.rs\n@@ -1,4 +1,3 @@\n-old\n+new\n context";
        let result = apply_plan(
            diff,
            &ReadingDiffPlan {
                remove: vec![LineRange { start: 3, end: 3 }],
                fold: Vec::new(),
                summary: "Changes the value.".into(),
            },
        )
        .unwrap();
        assert!(result.diff.contains("@@ -1,4 +1,3 @@"));
        assert!(!result.diff.contains("-old"));
        assert!(result.diff.contains("+new"));
    }

    #[test]
    fn rejects_metadata_targets() {
        let err = apply_plan(
            "@@ -1 +1 @@\n-old\n+new",
            &ReadingDiffPlan {
                remove: vec![LineRange { start: 1, end: 1 }],
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("metadata"));
    }
}
