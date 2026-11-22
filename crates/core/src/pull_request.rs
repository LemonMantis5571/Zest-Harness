//! Read-only GitHub pull request inspection for desktop frontends.
use crate::{PullRequestLink, ThreadGitContext, WorkspaceChangeSet};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Mutex,
    time::Duration,
};
use tokio::process::Command;

#[derive(Debug, Deserialize)]
struct GitHubPullRequest {
    number: u64,
    title: String,
    state: String,
    #[serde(rename = "isDraft")]
    is_draft: bool,
    url: String,
    additions: u64,
    deletions: u64,
    #[serde(rename = "changedFiles")]
    changed_files: u64,
}

#[derive(Clone)]
pub enum PullRequestLookup {
    Found(PullRequestLink),
    NotFound,
    Unavailable,
}

struct CachedPullRequest {
    checked: std::time::Instant,
    root: PathBuf,
    branch: String,
    lookup: PullRequestLookup,
}

static PULL_REQUEST_CACHE: Mutex<Option<CachedPullRequest>> = Mutex::new(None);

pub async fn lookup_pull_request_cached(root: &Path, branch: Option<&str>) -> PullRequestLookup {
    if let Some(branch) = branch.filter(|name| !name.is_empty()) {
        if let Ok(guard) = PULL_REQUEST_CACHE.lock() {
            if let Some(cached) = guard.as_ref() {
                if cached.root == root
                    && cached.branch == branch
                    && cached.checked.elapsed() < Duration::from_secs(30)
                {
                    return cached.lookup.clone();
                }
            }
        }
        let lookup = lookup_pull_request(root).await;
        if let Ok(mut guard) = PULL_REQUEST_CACHE.lock() {
            *guard = Some(CachedPullRequest {
                checked: std::time::Instant::now(),
                root: root.to_path_buf(),
                branch: branch.to_string(),
                lookup: lookup.clone(),
            });
        }
        return lookup;
    }
    lookup_pull_request(root).await
}

async fn lookup_pull_request(root: &Path) -> PullRequestLookup {
    let output = match tokio::time::timeout(
        Duration::from_secs(3),
        Command::new("gh")
            .args([
                "pr",
                "view",
                "--json",
                "number,title,state,isDraft,url,additions,deletions,changedFiles",
            ])
            .current_dir(root)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    {
        Ok(Ok(output)) => output,
        Ok(Err(_)) | Err(_) => return PullRequestLookup::Unavailable,
    };

    if !output.status.success() {
        return PullRequestLookup::NotFound;
    }
    let Ok(pr) = serde_json::from_slice::<GitHubPullRequest>(&output.stdout) else {
        return PullRequestLookup::Unavailable;
    };
    PullRequestLookup::Found(PullRequestLink {
        repository: None,
        number: pr.number,
        title: pr.title,
        url: pr.url,
        state: pr.state,
        is_draft: pr.is_draft,
        additions: pr.additions,
        deletions: pr.deletions,
        changed_files: pr.changed_files,
    })
}

async fn fetch_github_pull_request_diff(root: &Path, number: u64) -> Option<Vec<u8>> {
    let output = match tokio::time::timeout(
        Duration::from_secs(30),
        Command::new("gh")
            .args(["pr", "diff", &number.to_string()])
            .current_dir(root)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    {
        Ok(Ok(output)) => output,
        Ok(Err(_)) | Err(_) => return None,
    };
    output.status.success().then_some(output.stdout)
}

pub async fn diff(
    root: &Path,
    number: u64,
    context: Option<&ThreadGitContext>,
) -> Result<WorkspaceChangeSet, String> {
    if !crate::workspace_changes::is_safe_pr_number(number) {
        return Err("Invalid pull request number".into());
    }
    if let Some(raw) = fetch_github_pull_request_diff(root, number).await {
        return Ok(crate::snapshot_from_unified_diff(
            "pull_request",
            &raw,
            context.and_then(|context| context.base_branch.as_deref()),
            context.and_then(|context| context.branch.as_deref()),
        ));
    }
    let Some(base_branch) = context
        .and_then(|context| context.base_branch.as_deref())
        .filter(|value| crate::workspace_changes::is_safe_git_ref(value))
    else {
        return Ok(crate::WorkspaceChangeSet::unavailable("pull_request"));
    };
    crate::inspect_merge_base(root, base_branch)
        .await
        .map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct PullRequestStatus {
    pub state: String,
    pub url: String,
    pub checks: Vec<PullRequestCheck>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PullRequestCheck {
    pub name: String,
    pub status: String,
    pub url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPullRequest {
    state: String,
    url: String,
    status_check_rollup: Vec<GhCheck>,
}

#[derive(Deserialize)]
#[serde(tag = "__typename")]
enum GhCheck {
    CheckRun {
        name: String,
        status: String,
        conclusion: Option<String>,
        #[serde(rename = "detailsUrl")]
        url: Option<String>,
    },
    StatusContext {
        context: String,
        state: String,
        #[serde(rename = "targetUrl")]
        url: Option<String>,
    },
}

impl From<GhCheck> for PullRequestCheck {
    fn from(check: GhCheck) -> Self {
        let (name, state, url) = match check {
            GhCheck::CheckRun {
                name,
                status,
                conclusion,
                url,
            } => {
                let state = if status == "COMPLETED" {
                    conclusion.filter(|s| !s.is_empty()).unwrap_or(status)
                } else {
                    status
                };
                (name, state, url)
            }
            GhCheck::StatusContext {
                context,
                state,
                url,
            } => (context, state, url),
        };
        Self {
            name,
            status: state,
            url,
        }
    }
}

pub async fn status(root: &Path, number: u64) -> Result<PullRequestStatus, String> {
    if !crate::workspace_changes::is_safe_pr_number(number) {
        return Err("Invalid pull request number".into());
    }
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new("gh")
            .args([
                "pr",
                "view",
                &number.to_string(),
                "--json",
                "state,url,statusCheckRollup",
            ])
            .current_dir(root)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "GitHub check lookup timed out")?
    .map_err(|_| "Could not start the GitHub CLI")?;
    if !output.status.success() {
        return Err(
            "GitHub checks unavailable. Check the repository connection and gh sign-in.".into(),
        );
    }
    let pr: GhPullRequest = serde_json::from_slice(&output.stdout)
        .map_err(|_| "GitHub returned an unreadable check status")?;
    Ok(PullRequestStatus {
        state: pr.state,
        url: pr.url,
        checks: pr.status_check_rollup.into_iter().map(Into::into).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_running_and_completed_checks_and_external_statuses() {
        let pr: GhPullRequest = serde_json::from_value(serde_json::json!({
            "url":"https://github.com/zest/app/pull/13", "state":"MERGED",
            "statusCheckRollup":[
              {"__typename":"CheckRun","name":"Linux","status":"COMPLETED","conclusion":"FAILURE","detailsUrl":"https://github.com/check/1"},
              {"__typename":"CheckRun","name":"Windows","status":"IN_PROGRESS","conclusion":""},
              {"__typename":"StatusContext","context":"external","state":"SUCCESS","targetUrl":null}
            ]
        })).unwrap();
        let checks: Vec<PullRequestCheck> =
            pr.status_check_rollup.into_iter().map(Into::into).collect();
        assert_eq!(checks[0].status, "FAILURE");
        assert_eq!(checks[0].url.as_deref(), Some("https://github.com/check/1"));
        assert_eq!(checks[1].status, "IN_PROGRESS");
        assert_eq!(checks[2].name, "external");
        assert_eq!(checks[2].status, "SUCCESS");
    }
}
