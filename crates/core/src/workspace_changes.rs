//! Read-only Git workspace change inspection.
//!
//! This module owns the expensive and security-sensitive part of branch review:
//! comparing a thread's starting commit with the current checkout, adding
//! untracked files, redacting likely-secret paths, and producing a bounded
//! patch for the desktop. Callers do not need to know which Git commands or
//! path cases are involved.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::error::{HarnessError, Result};
use crate::tools::sensitive::is_sensitive_path;

/// Maximum patch payload sent to a desktop window in one change snapshot.
/// Large repositories still expose complete file/count metadata and mark the
/// visible patch as truncated.
pub const MAX_DISPLAY_DIFF_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeSummary {
    pub path: String,
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    pub binary: bool,
    pub sensitive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChangeSet {
    /// Stable identity for the effective checkout state used to suppress
    /// repeated auto-opening of the same review.
    pub change_id: String,
    pub repository: String,
    pub base_commit: Option<String>,
    pub base_branch: Option<String>,
    pub branch: Option<String>,
    pub changed_files: Vec<FileChangeSummary>,
    pub additions: u64,
    pub deletions: u64,
    pub diff: String,
    pub truncated: bool,
    pub unavailable: bool,
}

impl WorkspaceChangeSet {
    pub fn empty(repository: impl Into<String>) -> Self {
        Self {
            change_id: blake3::hash(b"clean").to_hex().to_string(),
            repository: repository.into(),
            base_commit: None,
            base_branch: None,
            branch: None,
            changed_files: Vec::new(),
            additions: 0,
            deletions: 0,
            diff: String::new(),
            truncated: false,
            unavailable: false,
        }
    }

    pub fn unavailable(repository: impl Into<String>) -> Self {
        let mut snapshot = Self::empty(repository);
        snapshot.change_id = blake3::hash(b"unavailable").to_hex().to_string();
        snapshot.unavailable = true;
        snapshot
    }

    pub fn has_changes(&self) -> bool {
        !self.changed_files.is_empty() || !self.diff.trim().is_empty()
    }
}

#[derive(Debug, Clone)]
struct StatusEntry {
    path: String,
    status: String,
    sensitive: bool,
}

/// Capture the cumulative branch/worktree patch for a thread.
pub async fn inspect(
    root: impl AsRef<Path>,
    base_commit: Option<&str>,
    base_branch: Option<&str>,
) -> Result<WorkspaceChangeSet> {
    let root = root.as_ref().to_path_buf();
    let probe = match run_git(&root, &["rev-parse", "--is-inside-work-tree"]).await {
        Ok(output) => output,
        Err(_) => return Ok(WorkspaceChangeSet::unavailable("git")),
    };
    if !probe.status.success() || String::from_utf8_lossy(&probe.stdout).trim() != "true" {
        return Ok(WorkspaceChangeSet::unavailable("not_git"));
    }

    let branch = git_branch(&root).await;
    let status = run_git(
        &root,
        &["status", "--porcelain=v1", "--untracked-files=all", "-z"],
    )
    .await?;
    if !status.status.success() {
        return Ok(WorkspaceChangeSet::unavailable("git"));
    }
    let entries = parse_status(&status.stdout);

    let tracked_args = diff_args(base_commit);
    let mut raw_diff = match run_git(&root, &tracked_args).await {
        Ok(tracked) if tracked.status.success() || tracked.status.code() == Some(1) => {
            tracked.stdout
        }
        // A repository without its first commit has no `HEAD`. Staged changes
        // are still useful review material, so use the index as the fallback.
        Ok(_) | Err(_) if base_commit.is_none() => run_git(&root, &diff_cached_args())
            .await
            .map(|output| output.stdout)
            .unwrap_or_default(),
        Ok(_) | Err(_) => return Ok(WorkspaceChangeSet::unavailable("git")),
    };
    for entry in entries.iter().filter(|entry| entry.status == "??") {
        if is_sensitive_path(&entry.path) {
            raw_diff.extend_from_slice(
                format!(
                    "diff --git a/{0} b/{0}\n--- /dev/null\n+++ b/{0}\n@@ sensitive file omitted @@\n",
                    entry.path
                )
                .as_bytes(),
            );
            continue;
        }
        let output = run_git(
            &root,
            &[
                "diff",
                "--no-index",
                "--binary",
                "--",
                "/dev/null",
                &entry.path,
            ],
        )
        .await?;
        // `git diff --no-index` returns 1 when the files differ, which is the
        // normal result for an untracked file.
        if output.status.success() || output.status.code() == Some(1) {
            raw_diff.extend_from_slice(&output.stdout);
        }
    }

    let change_id = change_id(&root, base_commit, &status.stdout, &raw_diff).await;
    let redacted = redact_diff(&raw_diff);
    let (diff, truncated) = bounded_utf8(&redacted, MAX_DISPLAY_DIFF_BYTES);
    let mut summaries = summaries_from_status(&entries);
    fill_stats_from_diff(&mut summaries, &raw_diff);

    Ok(WorkspaceChangeSet {
        change_id,
        repository: "git".into(),
        base_commit: base_commit.map(str::to_string),
        base_branch: base_branch.map(str::to_string),
        branch,
        additions: summaries.iter().map(|file| file.additions).sum(),
        deletions: summaries.iter().map(|file| file.deletions).sum(),
        changed_files: summaries,
        diff,
        truncated,
        unavailable: false,
    })
}

fn diff_args(base_commit: Option<&str>) -> Vec<&str> {
    let mut args = vec!["diff", "--no-ext-diff", "--find-renames", "--binary"];
    if let Some(base_commit) = base_commit.filter(|value| !value.trim().is_empty()) {
        args.push(base_commit);
    } else {
        // Comparing against HEAD includes both staged and unstaged tracked
        // changes. Untracked paths are appended separately below.
        args.push("HEAD");
    }
    args.extend(["--"]);
    args
}

fn diff_cached_args() -> Vec<&'static str> {
    vec![
        "diff",
        "--no-ext-diff",
        "--find-renames",
        "--binary",
        "--cached",
        "--",
    ]
}

async fn run_git(root: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .await
        .map_err(|error| HarnessError::Other(format!("could not run git: {error}")))
}

async fn git_branch(root: &Path) -> Option<String> {
    let output = run_git(root, &["symbolic-ref", "--short", "-q", "HEAD"])
        .await
        .ok()?;
    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !branch.is_empty() {
            return Some(branch);
        }
    }
    let output = run_git(root, &["rev-parse", "--short", "HEAD"])
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn parse_status(raw: &[u8]) -> Vec<StatusEntry> {
    let mut fields = raw.split(|byte| *byte == 0);
    let mut entries = Vec::new();
    while let Some(field) = fields.next() {
        if field.len() < 3 {
            continue;
        }
        let text = String::from_utf8_lossy(field);
        let code = &text[..2];
        let mut path = text[3..].to_string();
        // Porcelain -z emits the destination path as the following field for
        // renames/copies. The destination is the path users need to review.
        if code.contains('R') || code.contains('C') {
            if let Some(destination) = fields.next() {
                path = String::from_utf8_lossy(destination).to_string();
            }
        }
        let status = if code == "??" {
            "untracked".to_string()
        } else if code.contains('D') {
            "deleted".to_string()
        } else if code.contains('R') {
            "renamed".to_string()
        } else if code.contains('A') {
            "added".to_string()
        } else {
            "modified".to_string()
        };
        entries.push(StatusEntry {
            sensitive: is_sensitive_path(&path),
            path,
            status,
        });
    }
    entries
}

fn summaries_from_status(entries: &[StatusEntry]) -> Vec<FileChangeSummary> {
    entries
        .iter()
        .map(|entry| FileChangeSummary {
            path: entry.path.clone(),
            status: entry.status.clone(),
            additions: 0,
            deletions: 0,
            binary: false,
            sensitive: entry.sensitive,
        })
        .collect()
}

fn fill_stats_from_diff(summaries: &mut Vec<FileChangeSummary>, raw: &[u8]) {
    let text = String::from_utf8_lossy(raw);
    let mut current: Option<usize> = None;
    for line in text.lines() {
        if let Some(path) = diff_path(line) {
            current = summaries.iter().position(|file| file.path == path);
            if current.is_none() {
                let status = if line.contains("/dev/null") {
                    "added"
                } else {
                    "modified"
                };
                summaries.push(FileChangeSummary {
                    sensitive: is_sensitive_path(&path),
                    path,
                    status: status.into(),
                    additions: 0,
                    deletions: 0,
                    binary: false,
                });
                current = Some(summaries.len() - 1);
            }
        }
        let Some(index) = current else { continue };
        if line.starts_with("Binary files") || line.starts_with("GIT binary patch") {
            summaries[index].binary = true;
        } else if line.starts_with('+') && !line.starts_with("+++") {
            summaries[index].additions = summaries[index].additions.saturating_add(1);
        } else if line.starts_with('-') && !line.starts_with("---") {
            summaries[index].deletions = summaries[index].deletions.saturating_add(1);
        }
    }
}

fn diff_path(line: &str) -> Option<String> {
    let rest = line.strip_prefix("diff --git a/")?;
    let (left, right) = rest.split_once(" b/")?;
    Some(if !right.is_empty() { right } else { left }.to_string())
}

fn redact_diff(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let mut output = String::new();
    let mut omitted_sensitive_body = false;
    for line in text.lines() {
        if let Some(path) = diff_path(line) {
            output.push_str(line);
            output.push('\n');
            omitted_sensitive_body = is_sensitive_path(&path);
            if omitted_sensitive_body {
                output.push_str("@@ sensitive file omitted @@\n");
            }
            continue;
        }
        if omitted_sensitive_body {
            continue;
        }
        output.push_str(line);
        output.push('\n');
    }
    output
}

fn bounded_utf8(raw: &str, limit: usize) -> (String, bool) {
    if raw.len() <= limit {
        return (raw.to_string(), false);
    }
    let mut end = limit;
    while end > 0 && !raw.is_char_boundary(end) {
        end -= 1;
    }
    let mut value = raw[..end].to_string();
    value.push_str("\n\n[Diff truncated; file and line counts remain complete.]\n");
    (value, true)
}

async fn change_id(
    root: &Path,
    base_commit: Option<&str>,
    status: &[u8],
    raw_diff: &[u8],
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(root.to_string_lossy().as_bytes());
    hasher.update(base_commit.unwrap_or_default().as_bytes());
    hasher.update(status);
    hasher.update(raw_diff);
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_parser_handles_untracked_and_renamed_entries() {
        let raw = b"?? new.txt\0R  old.txt\0new-name.txt\0";
        let entries = parse_status(raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].status, "untracked");
        assert_eq!(entries[1].path, "new-name.txt");
        assert_eq!(entries[1].status, "renamed");
    }

    #[test]
    fn sensitive_sections_are_replaced_without_leaking_body() {
        let diff = "diff --git a/.env b/.env\n+++ b/.env\n@@ -1 +1 @@\n-SECRET\n+NEW_SECRET\ndiff --git a/src/lib.rs b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let redacted = redact_diff(diff.as_bytes());
        assert!(redacted.contains("sensitive file omitted"));
        assert!(!redacted.contains("SECRET"));
        assert!(redacted.contains("src/lib.rs"));
    }

    #[test]
    fn bounded_diff_marks_truncation() {
        let (value, truncated) = bounded_utf8("abcdef", 3);
        assert!(truncated);
        assert!(value.starts_with("abc"));
        assert!(value.contains("Diff truncated"));
    }
}
