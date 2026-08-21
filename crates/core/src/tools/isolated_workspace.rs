//! Shared isolated-workspace lifecycle for delegated workers.
//!
//! This module owns the Git boundary only: creating and removing temporary
//! worktrees, copying the caller's dirty snapshot, hiding sensitive files,
//! seeding a reviewer with a worker patch, and collecting a safe diff.
//! Process transport and provider/CLI output parsing remain in the caller so
//! native provider workers can reuse this seam later.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::sensitive::is_sensitive_path;

const MAX_ERROR_CHARS: usize = 2_000;

/// A prepared isolated worktree and the baseline used to distinguish worker
/// edits from the user's pre-existing dirty state.
pub(crate) struct PreparedWorkspace {
    _temp: tempfile::TempDir,
    guard: WorktreeGuard,
    root: PathBuf,
    source_root: PathBuf,
    baseline: String,
}

impl PreparedWorkspace {
    pub(crate) fn path(&self) -> &Path {
        &self.root
    }

    pub(crate) async fn collect_diff(&self) -> Result<String, String> {
        collect_git_diff_with_untracked(&self.root, &self.source_root, &self.baseline).await
    }

    pub(crate) async fn cleanup(&mut self) -> Result<(), String> {
        self.guard.cleanup().await
    }
}

/// Prepare a worker from the current project snapshot.
pub(crate) async fn prepare(root: &Path) -> Result<PreparedWorkspace, String> {
    let base = git_output(root, &["rev-parse", "HEAD"]).await?;
    let temp = tempfile::tempdir().map_err(|error| format!("create worktree temp dir: {error}"))?;
    let worktree = temp.path().join("workspace");
    git_output_args(
        root,
        &["worktree", "add", "--detach", "--quiet"],
        Some(&worktree),
        Some(&base),
    )
    .await
    .map_err(|error| format!("create isolated worktree: {error}"))?;

    let mut guard = WorktreeGuard::new(root, &worktree);
    if let Err(error) = remove_sensitive_tracked_files(&worktree).await {
        let _ = guard.cleanup().await;
        return Err(error);
    }
    if let Err(error) = copy_working_snapshot(root, &worktree, &base).await {
        let _ = guard.cleanup().await;
        return Err(error);
    }
    let baseline = match snapshot_worktree(&worktree).await {
        Ok(value) => value,
        Err(error) => {
            let _ = guard.cleanup().await;
            return Err(error);
        }
    };

    Ok(PreparedWorkspace {
        _temp: temp,
        guard,
        root: worktree,
        source_root: root.to_path_buf(),
        baseline,
    })
}

/// Prepare a reviewer workspace and apply the worker patch before taking the
/// reviewer baseline. Reviewer edits are returned separately from the worker
/// patch and can be discarded safely.
pub(crate) async fn prepare_reviewer(
    root: &Path,
    worker_diff: &str,
) -> Result<PreparedWorkspace, String> {
    if !worker_diff.trim().is_empty() {
        crate::delegation::validate_diff_paths(root, worker_diff)
            .map_err(|error| format!("worker diff is unsafe: {error}"))?;
    }
    let mut workspace = prepare(root).await?;
    if !worker_diff.trim().is_empty() {
        if let Err(error) = apply_diff_to_workspace(workspace.path(), worker_diff).await {
            let _ = workspace.cleanup().await;
            return Err(error);
        }
    }
    workspace.baseline = match snapshot_worktree(workspace.path()).await {
        Ok(value) => value,
        Err(error) => {
            let _ = workspace.cleanup().await;
            return Err(error);
        }
    };
    Ok(workspace)
}

pub(crate) async fn apply_diff_to_workspace(worktree: &Path, diff: &str) -> Result<(), String> {
    let mut command = Command::new("git");
    command
        .args(["apply", "--binary", "--whitespace=nowarn", "-"])
        .current_dir(worktree)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("start worker diff apply: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(diff.as_bytes())
            .await
            .map_err(|error| format!("write worker diff: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .await
        .map_err(|error| format!("apply worker diff: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "could not apply worker diff: {}",
            clip(String::from_utf8_lossy(&output.stderr).trim())
        ))
    }
}

struct WorktreeGuard {
    root: PathBuf,
    worktree: PathBuf,
    active: bool,
}

impl WorktreeGuard {
    fn new(root: &Path, worktree: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            worktree: worktree.to_path_buf(),
            active: true,
        }
    }

    async fn cleanup(&mut self) -> Result<(), String> {
        if !self.active {
            return Ok(());
        }
        remove_worktree(&self.root, &self.worktree).await?;
        self.active = false;
        Ok(())
    }
}

impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        // Cancellation can drop the future before async cleanup runs.
        let _ = std::process::Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&self.worktree)
            .current_dir(&self.root)
            .output();
    }
}

async fn remove_worktree(root: &Path, worktree: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(worktree)
        .current_dir(root)
        .output()
        .await
        .map_err(|error| format!("could not start git cleanup: {error}"))?;
    if !output.status.success() {
        return Err(clip(String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(())
}

async fn remove_sensitive_tracked_files(root: &Path) -> Result<(), String> {
    let tracked = git_bytes(root, &["ls-files", "-z"]).await?;
    for raw in tracked.split(|byte| *byte == 0) {
        if raw.is_empty() {
            continue;
        }
        let relative = String::from_utf8_lossy(raw).replace('\\', "/");
        if !is_sensitive_path(&relative) {
            continue;
        }
        let path = root.join(&relative);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
                std::fs::remove_file(&path)
                    .map_err(|error| format!("remove sensitive worker file {relative}: {error}"))?;
            }
            Ok(metadata) if metadata.is_dir() => {
                std::fs::remove_dir_all(&path).map_err(|error| {
                    format!("remove sensitive worker directory {relative}: {error}")
                })?;
            }
            _ => {}
        }
    }
    Ok(())
}

async fn copy_working_snapshot(root: &Path, worktree: &Path, base: &str) -> Result<(), String> {
    let patch = safe_tracked_diff(root, base).await?;
    if !patch.is_empty() {
        let mut command = Command::new("git");
        command
            .args(["apply", "--binary", "-"])
            .current_dir(worktree)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| format!("could not apply current changes: {error}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(&patch)
                .await
                .map_err(|error| format!("write current changes: {error}"))?;
        }
        let output = child
            .wait_with_output()
            .await
            .map_err(|error| format!("apply current changes: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "could not apply current tracked changes: {}",
                clip(String::from_utf8_lossy(&output.stderr).trim())
            ));
        }
    }

    let untracked = git_bytes(root, &["ls-files", "--others", "--exclude-standard", "-z"]).await?;
    for raw in untracked.split(|byte| *byte == 0) {
        if raw.is_empty() {
            continue;
        }
        let relative = String::from_utf8_lossy(raw).replace('\\', "/");
        if relative == ".zest" || relative.starts_with(".zest/") || is_sensitive_path(&relative) {
            continue;
        }
        let source = root.join(&relative);
        let metadata = std::fs::symlink_metadata(&source)
            .map_err(|error| format!("inspect untracked file {relative}: {error}"))?;
        if !metadata.file_type().is_file() {
            continue;
        }
        let destination = worktree.join(&relative);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("copy untracked directory {relative}: {error}"))?;
        }
        std::fs::copy(&source, &destination)
            .map_err(|error| format!("copy untracked file {relative}: {error}"))?;
    }
    Ok(())
}

async fn snapshot_worktree(root: &Path) -> Result<String, String> {
    git_output(root, &["add", "-A", "--", "."]).await?;
    git_output(
        root,
        &[
            "-c",
            "user.name=Zest",
            "-c",
            "user.email=zest@localhost",
            "-c",
            "commit.gpgSign=false",
            "commit",
            "--allow-empty",
            "--no-verify",
            "--quiet",
            "-m",
            "Zest external delegation baseline",
        ],
    )
    .await?;
    git_output(root, &["rev-parse", "HEAD"]).await
}

async fn collect_git_diff_with_untracked(
    root: &Path,
    source_root: &Path,
    base: &str,
) -> Result<String, String> {
    let mut diff = String::from_utf8_lossy(&safe_tracked_diff(root, base).await?).to_string();
    let current_untracked =
        git_bytes(root, &["ls-files", "--others", "--exclude-standard", "-z"]).await?;
    for raw in current_untracked.split(|byte| *byte == 0) {
        if raw.is_empty() {
            continue;
        }
        let relative = String::from_utf8_lossy(raw).replace('\\', "/");
        if relative == ".zest" || relative.starts_with(".zest/") || is_sensitive_path(&relative) {
            continue;
        }
        let path = root.join(&relative);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_file() => metadata,
            _ => continue,
        };
        if metadata.len() > 2_000_000 {
            diff.push_str(&format!("\nBinary or large untracked file: {relative}\n"));
            continue;
        }
        let source = source_root.join(&relative);
        let output = if source.is_file() {
            git_no_index_diff(root, &source, &path, &relative).await?
        } else {
            git_no_index_diff(root, null_device(), &path, &relative).await?
        };
        if !output.is_empty() {
            diff.push_str(&output);
        }
    }

    let source_untracked = git_bytes(
        source_root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )
    .await?;
    for raw in source_untracked.split(|byte| *byte == 0) {
        if raw.is_empty() {
            continue;
        }
        let relative = String::from_utf8_lossy(raw).replace('\\', "/");
        if relative == ".zest" || relative.starts_with(".zest/") || is_sensitive_path(&relative) {
            continue;
        }
        let source = source_root.join(&relative);
        let target = root.join(&relative);
        if source.is_file() && !target.exists() && !git_path_is_tracked(root, &relative).await? {
            let output = git_no_index_diff(root, &source, null_device(), &relative).await?;
            if !output.is_empty() {
                diff.push_str(&output);
            }
        }
    }
    Ok(diff)
}

async fn git_path_is_tracked(root: &Path, relative: &str) -> Result<bool, String> {
    let output = Command::new("git")
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(relative)
        .current_dir(root)
        .output()
        .await
        .map_err(|error| format!("could not inspect tracked path: {error}"))?;
    if output.status.success() {
        return Ok(true);
    }
    if output.status.code() == Some(1) {
        return Ok(false);
    }
    Err(clip(String::from_utf8_lossy(&output.stderr).trim()))
}

async fn safe_tracked_diff(root: &Path, base: &str) -> Result<Vec<u8>, String> {
    let paths = git_bytes(root, &["diff", "--name-only", "-z", base, "--"]).await?;
    let safe_paths: Vec<String> = paths
        .split(|byte| *byte == 0)
        .filter(|raw| !raw.is_empty())
        .map(|raw| String::from_utf8_lossy(raw).replace('\\', "/"))
        .filter(|relative| !is_sensitive_path(relative))
        .collect();
    if safe_paths.is_empty() {
        return Ok(Vec::new());
    }
    let output = Command::new("git")
        .args(["diff", "--binary", "--no-ext-diff", base, "--"])
        .args(&safe_paths)
        .current_dir(root)
        .output()
        .await
        .map_err(|error| format!("could not start git diff: {error}"))?;
    if !output.status.success() {
        return Err(clip(String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(output.stdout)
}

async fn git_no_index_diff(
    cwd: &Path,
    left: &Path,
    right: &Path,
    relative: &str,
) -> Result<String, String> {
    let output = Command::new("git")
        .args(["diff", "--no-index", "--no-ext-diff", "--"])
        .arg(left)
        .arg(right)
        .current_dir(cwd)
        .output()
        .await
        .map_err(|error| format!("diff untracked file: {error}"))?;
    if output.status.success() || output.status.code() == Some(1) {
        return Ok(normalize_no_index_diff(
            &String::from_utf8_lossy(&output.stdout),
            relative,
        ));
    }
    Err(clip(String::from_utf8_lossy(&output.stderr).trim()))
}

pub(crate) fn normalize_no_index_diff(raw: &str, relative: &str) -> String {
    let relative = relative.replace('\\', "/");
    let mut normalized = String::with_capacity(raw.len());
    for line in raw.lines() {
        let replacement = if line.starts_with("diff --git ") {
            Some(format!("diff --git a/{relative} b/{relative}"))
        } else if line.starts_with("--- ") && !line.starts_with("--- /dev/null") {
            Some(format!("--- a/{relative}"))
        } else if line.starts_with("+++ ") && !line.starts_with("+++ /dev/null") {
            Some(format!("+++ b/{relative}"))
        } else {
            None
        };
        normalized.push_str(replacement.as_deref().unwrap_or(line));
        normalized.push('\n');
    }
    if !raw.ends_with('\n') {
        normalized.pop();
    }
    normalized
}

fn null_device() -> &'static Path {
    if cfg!(windows) {
        Path::new("NUL")
    } else {
        Path::new("/dev/null")
    }
}

pub(crate) async fn git_output(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .map_err(|error| format!("could not start git: {error}"))?;
    if !output.status.success() {
        return Err(clip(String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub(crate) async fn collect_git_diff(root: &Path, base: &str) -> Result<String, String> {
    Ok(String::from_utf8_lossy(&safe_tracked_diff(root, base).await?).to_string())
}

async fn git_output_args(
    cwd: &Path,
    args: &[&str],
    path: Option<&Path>,
    trailing: Option<&str>,
) -> Result<String, String> {
    let mut command = Command::new("git");
    command.args(args).current_dir(cwd);
    if let Some(path) = path {
        command.arg(path);
    }
    if let Some(value) = trailing {
        command.arg(value);
    }
    let output = command
        .output()
        .await
        .map_err(|error| format!("could not start git: {error}"))?;
    if !output.status.success() {
        return Err(clip(String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn git_bytes(cwd: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .map_err(|error| format!("could not start git: {error}"))?;
    if !output.status.success() {
        return Err(clip(String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(output.stdout)
}

fn clip(value: &str) -> String {
    if value.chars().count() <= MAX_ERROR_CHARS {
        return value.to_string();
    }
    let clipped: String = value.chars().take(MAX_ERROR_CHARS - 1).collect();
    format!("{clipped}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn git_ok(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repo() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "base\n").unwrap();
        std::fs::write(temp.path().join(".env"), "SECRET=must-not-leak\n").unwrap();
        temp
    }

    async fn committed_repo() -> tempfile::TempDir {
        let temp = repo();
        git_ok(temp.path(), &["init", "--quiet"]).await;
        git_ok(temp.path(), &["config", "user.name", "Test"]).await;
        git_ok(
            temp.path(),
            &["config", "user.email", "test@example.invalid"],
        )
        .await;
        git_ok(temp.path(), &["add", "."]).await;
        git_ok(
            temp.path(),
            &["commit", "--quiet", "--no-verify", "-m", "base"],
        )
        .await;
        temp
    }

    #[tokio::test]
    async fn prepared_workspace_removes_sensitive_tracked_files() {
        let temp = committed_repo().await;
        let mut workspace = prepare(temp.path()).await.unwrap();

        assert!(workspace.path().join("tracked.txt").exists());
        assert!(!workspace.path().join(".env").exists());

        workspace.cleanup().await.unwrap();
        assert!(!workspace.path().exists());
    }

    #[tokio::test]
    async fn cleanup_unregisters_the_temporary_worktree() {
        let temp = committed_repo().await;
        let mut workspace = prepare(temp.path()).await.unwrap();
        let worktree = workspace.path().to_path_buf();

        workspace.cleanup().await.unwrap();

        let output = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(temp.path())
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        assert!(!String::from_utf8_lossy(&output.stdout).contains(&worktree.display().to_string()));
    }

    #[tokio::test]
    async fn reviewer_rejects_unsafe_diff_before_creating_a_worktree() {
        let temp = committed_repo().await;

        let error = match prepare_reviewer(temp.path(), "diff --git a/.env b/.env\n").await {
            Ok(_) => panic!("sensitive worker diff unexpectedly accepted"),
            Err(error) => error,
        };

        assert!(error.contains("worker diff is unsafe"), "{error}");
        let output = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(temp.path())
            .output()
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|line| line.starts_with("worktree "))
                .count(),
            1
        );
    }
}
