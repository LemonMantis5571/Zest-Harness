//! Optional Jev checks for completed plans and workspace changes.

use std::path::Path;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::State;
use zest_core::tools::jev::{check_with_jev, configured_review_model, JevCheck, JevReviewKind};

use super::{config_for_session, review_workspace_at, zest_config_dir, AppState};

const MAX_STATE_BYTES: usize = 80 * 1024;
static REVIEW_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct JevQuickReviewView {
    pub target_id: String,
    pub content_hash: String,
    pub kind: String,
    /// `clear`, `attention`, `inconclusive`, `unavailable`, or `skipped`.
    pub status: String,
    pub checks: Vec<JevCheck>,
    pub model: String,
    pub usage: Option<Value>,
    pub source_excerpt: String,
    pub detail: Option<String>,
    pub reviewed_at_ms: u64,
}

fn review_status(checks: &[JevCheck]) -> &'static str {
    if checks.is_empty() {
        "inconclusive"
    } else if checks.iter().any(|check| check.outcome == "concern") {
        "attention"
    } else if checks.iter().any(|check| check.outcome == "inconclusive") {
        "inconclusive"
    } else {
        "clear"
    }
}

fn workspace_skip_reason(
    changes: &zest_core::workspace_changes::WorkspaceChangeSet,
) -> Option<&'static str> {
    if changes.unavailable || changes.truncated || changes.diff.trim().is_empty() {
        Some("The diff is empty, unavailable, or truncated.")
    } else if changes.changed_files.iter().any(|file| file.sensitive) {
        Some("The diff contains a file marked sensitive.")
    } else {
        None
    }
}

fn review_path(
    root: &Path,
    kind: &str,
    target_id: &str,
    content_hash: &str,
    model: &str,
) -> Result<std::path::PathBuf, String> {
    let root_id = blake3::hash(root.to_string_lossy().as_bytes())
        .to_hex()
        .to_string();
    let key = format!("{kind}\0{target_id}\0{content_hash}\0{model}");
    let review_id = blake3::hash(key.as_bytes()).to_hex();
    Ok(zest_config_dir()?
        .join("jev-reviews")
        .join(root_id)
        .join(format!("{review_id}.json")))
}

fn excerpt(text: &str) -> String {
    let mut value: String = text.trim().chars().take(260).collect();
    if text.trim().chars().count() > 260 {
        value.push('…');
    }
    value
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

async fn run_review(
    root: &Path,
    config: &zest_core::Config,
    kind: JevReviewKind,
    target_id: String,
    state: Value,
    source_excerpt: String,
    skip_reason: Option<&str>,
    force: bool,
    cache_only: bool,
) -> Result<Option<JevQuickReviewView>, String> {
    let Some(model) = configured_review_model(config) else {
        return Ok(None);
    };
    let kind_name = match kind {
        JevReviewKind::Plan => "plan",
        JevReviewKind::Changes => "changes",
        JevReviewKind::Delegated => "delegated",
    };
    let encoded = serde_json::to_vec(&state).map_err(|error| error.to_string())?;
    let content_hash = blake3::hash(&encoded).to_hex().to_string();
    let path = review_path(root, kind_name, &target_id, &content_hash, model)?;
    let lock = REVIEW_LOCK.get_or_init(|| tokio::sync::Mutex::new(()));
    let _guard = lock.lock().await;
    if !force {
        if let Ok(bytes) = tokio::fs::read(&path).await {
            if let Ok(cached) = serde_json::from_slice::<JevQuickReviewView>(&bytes) {
                return Ok(Some(cached));
            }
        }
    }
    if cache_only {
        return Ok(None);
    }
    let skip = skip_reason.map(str::to_string).or_else(|| {
        (encoded.len() > MAX_STATE_BYTES)
            .then(|| "The evidence is too large for a reliable quick check.".to_string())
    });
    let (status, checks, actual_model, usage, detail) = if let Some(reason) = skip {
        (
            "skipped".to_string(),
            Vec::new(),
            model.to_string(),
            None,
            Some(reason),
        )
    } else {
        match check_with_jev(config, kind, state).await {
            Ok(Some(review)) => (
                review_status(&review.checks).to_string(),
                review.checks,
                review.model,
                review.usage,
                None,
            ),
            Ok(None) => return Ok(None),
            Err(error) => (
                "unavailable".to_string(),
                Vec::new(),
                model.to_string(),
                None,
                Some(error),
            ),
        }
    };
    let view = JevQuickReviewView {
        target_id,
        content_hash,
        kind: kind_name.into(),
        status,
        checks,
        model: actual_model,
        usage,
        source_excerpt,
        detail,
        reviewed_at_ms: now_ms(),
    };
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| error.to_string())?;
    }
    tokio::fs::write(
        path,
        serde_json::to_vec_pretty(&view).map_err(|error| error.to_string())?,
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(Some(view))
}

#[tauri::command]
pub(super) async fn check_jev_plan(
    state: State<'_, AppState>,
    thread_id: String,
    message_id: String,
    request: String,
    plan: String,
    force: bool,
    cache_only: bool,
) -> Result<Option<JevQuickReviewView>, String> {
    let (root, active_thread_id) = state
        .sessions
        .session_info_snapshot(|session| (session.root.clone(), session.thread_id.clone()))
        .map_err(super::map_session_err)?
        .ok_or_else(|| "no active chat".to_string())?;
    if thread_id != active_thread_id {
        return Err("the active chat changed before Jev could check the plan".into());
    }
    if message_id.trim().is_empty() || request.trim().is_empty() || plan.trim().is_empty() {
        return Err("a finished plan and its request are required".into());
    }
    let config = config_for_session(&state, &root)?;
    let source_excerpt = excerpt(&request);
    run_review(
        &root,
        &config,
        JevReviewKind::Plan,
        format!("{thread_id}:{message_id}"),
        json!({"request": request, "plan": plan}),
        source_excerpt,
        None,
        force,
        cache_only,
    )
    .await
}

#[tauri::command]
pub(super) async fn check_jev_changes(
    state: State<'_, AppState>,
    thread_id: String,
    change_id: String,
    objective: String,
    claimed_summary: String,
    force: bool,
    cache_only: bool,
) -> Result<Option<JevQuickReviewView>, String> {
    let snapshot = state
        .sessions
        .session_info_snapshot(|session| {
            (
                session.root.clone(),
                session.thread_id.clone(),
                session.thread.git_context.clone(),
            )
        })
        .map_err(super::map_session_err)?
        .ok_or_else(|| "no active chat".to_string())?;
    let (root, active_thread_id, context) = snapshot;
    if thread_id != active_thread_id {
        return Err("the active chat changed before Jev could check the changes".into());
    }
    let config = config_for_session(&state, &root)?;
    if configured_review_model(&config).is_none() {
        return Ok(None);
    }
    let changes = zest_core::workspace_changes::inspect(
        &root,
        context
            .as_ref()
            .and_then(|value| value.start_commit.as_deref()),
        context
            .as_ref()
            .and_then(|value| value.base_branch.as_deref()),
    )
    .await
    .map_err(|error| error.to_string())?;
    if changes.change_id != change_id {
        return Err("the workspace changed before Jev could check this diff".into());
    }
    let skip = workspace_skip_reason(&changes);
    let local_review = review_workspace_at(&root).await?;
    let current = zest_core::workspace_changes::inspect(
        &root,
        context
            .as_ref()
            .and_then(|value| value.start_commit.as_deref()),
        context
            .as_ref()
            .and_then(|value| value.base_branch.as_deref()),
    )
    .await
    .map_err(|error| error.to_string())?;
    if current.change_id != change_id {
        return Err("the workspace changed while Jev's evidence was prepared".into());
    }
    let source_excerpt = excerpt(&objective);
    run_review(
        &root,
        &config,
        JevReviewKind::Changes,
        format!("{thread_id}:{change_id}"),
        json!({"objective": objective, "claimedSummary": claimed_summary, "diff": changes.diff,
            "localPatchCheck": local_review.patch_check, "localSummary": local_review.summary}),
        source_excerpt,
        skip,
        force,
        cache_only,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_status_never_calls_uncertain_evidence_clear() {
        let check = |outcome: &str| JevCheck {
            id: "coverage".into(),
            label: "Coverage".into(),
            choice: outcome.into(),
            outcome: outcome.into(),
            probability: 0.91,
        };
        assert_eq!(review_status(&[check("clear")]), "clear");
        assert_eq!(review_status(&[check("inconclusive")]), "inconclusive");
        assert_eq!(
            review_status(&[check("clear"), check("concern")]),
            "attention"
        );
    }

    #[test]
    fn incomplete_or_sensitive_diffs_never_receive_a_clear_check() {
        let mut changes = zest_core::workspace_changes::WorkspaceChangeSet::empty("git");
        assert!(workspace_skip_reason(&changes).is_some());
        changes.diff = "+safe edit".into();
        assert!(workspace_skip_reason(&changes).is_none());
        changes.truncated = true;
        assert!(workspace_skip_reason(&changes).is_some());
        changes.truncated = false;
        changes
            .changed_files
            .push(zest_core::workspace_changes::FileChangeSummary {
                path: ".env".into(),
                status: "modified".into(),
                additions: 1,
                deletions: 0,
                binary: false,
                sensitive: true,
            });
        assert!(workspace_skip_reason(&changes).is_some());
    }
}
