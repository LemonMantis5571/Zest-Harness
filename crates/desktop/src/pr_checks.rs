use super::{resolve_pull_request_number, resolve_workspace_root, AppState};
use tauri::State;
pub(crate) use zest_core::pull_request::PullRequestStatus;

#[tauri::command]
pub(crate) async fn pull_request_status(
    state: State<'_, AppState>,
    number: u64,
) -> Result<PullRequestStatus, String> {
    let number =
        resolve_pull_request_number(Some(number), None).ok_or("Invalid pull request number")?;
    let root = resolve_workspace_root(&state)?;
    zest_core::pull_request::status(&root, number).await
}
