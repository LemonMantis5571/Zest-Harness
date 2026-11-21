use super::{resolve_pull_request_number, resolve_workspace_root, AppState};
use serde::{Deserialize, Serialize};
use std::{process::Stdio, time::Duration};
use tauri::State;
use tokio::process::Command;

#[derive(Serialize)]
#[cfg_attr(feature = "export-bindings", derive(ts_rs::TS))]
pub(crate) struct PullRequestStatus {
    state: String,
    url: String,
    checks: Vec<PullRequestCheck>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "export-bindings", derive(ts_rs::TS))]
struct PullRequestCheck {
    name: String,
    status: String,
    url: Option<String>,
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

#[tauri::command]
pub(crate) async fn pull_request_status(
    state: State<'_, AppState>,
    number: u64,
) -> Result<PullRequestStatus, String> {
    let number =
        resolve_pull_request_number(Some(number), None).ok_or("Invalid pull request number")?;
    let root = resolve_workspace_root(&state)?;
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
