//! Persistent Git worktrees for independent coding chats.
//!
//! Delegated workers use disposable worktrees and copy the caller's dirty
//! snapshot. A chat worktree is different: it starts at a committed HEAD and
//! stays on disk until the user deliberately removes it.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::thread::new_id;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatWorktree {
    pub path: PathBuf,
    pub branch: String,
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| format!("could not start Git: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Git failed: {}", detail.trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Create a new branch and checkout outside the source repository. The caller
/// supplies an app-owned storage directory and records the returned path.
/// Existing files, branches, and working-tree changes are never overwritten.
pub fn create_chat_worktree(source: &Path, storage: &Path) -> Result<ChatWorktree, String> {
    let source = source
        .canonicalize()
        .map_err(|error| format!("cannot open project: {error}"))?;
    let top_level = git(&source, &["rev-parse", "--show-toplevel"])
        .map_err(|_| "Worktree chats need a Git repository with a commit.".to_string())?;
    let top_level = PathBuf::from(top_level)
        .canonicalize()
        .map_err(|error| format!("cannot locate repository root: {error}"))?;
    if source != top_level {
        return Err("Open the Git repository root to start a worktree chat.".into());
    }
    let head = git(&source, &["rev-parse", "--verify", "HEAD"])
        .map_err(|_| "Commit this repository once before starting a worktree chat.".to_string())?;
    std::fs::create_dir_all(storage)
        .map_err(|error| format!("cannot prepare worktree storage: {error}"))?;
    let id = new_id("chat");
    let path = storage.join(&id);
    let branch = format!("zest/{id}");
    let output = Command::new("git")
        .args(["worktree", "add", "-b", &branch, "--"])
        .arg(&path)
        .arg(&head)
        .current_dir(&source)
        .output()
        .map_err(|error| format!("could not start Git worktree creation: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Could not create worktree: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(ChatWorktree { path, branch })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(root: &Path, args: &[&str]) {
        git(root, args).unwrap();
    }

    #[test]
    fn chats_get_independent_branches_and_leave_the_source_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        run(&source, &["init", "-q"]);
        std::fs::write(source.join("shared.txt"), "base\n").unwrap();
        run(&source, &["add", "shared.txt"]);
        run(
            &source,
            &[
                "-c",
                "user.name=Zest Test",
                "-c",
                "user.email=zest@example.invalid",
                "commit",
                "-qm",
                "base",
            ],
        );
        std::fs::write(source.join("local-only.txt"), "do not copy").unwrap();

        let first = create_chat_worktree(&source, &temp.path().join("worktrees")).unwrap();
        let second = create_chat_worktree(&source, &temp.path().join("worktrees")).unwrap();
        assert_ne!(first.path, second.path);
        assert_ne!(first.branch, second.branch);
        assert_eq!(
            git(&first.path, &["branch", "--show-current"]).unwrap(),
            first.branch
        );
        assert!(
            crate::prompt::env_context(&first.path).contains(&format!("branch {}", first.branch))
        );
        assert_eq!(
            git(&second.path, &["branch", "--show-current"]).unwrap(),
            second.branch
        );
        assert!(!first.path.join("local-only.txt").exists());
        assert!(!second.path.join("local-only.txt").exists());

        std::fs::write(first.path.join("shared.txt"), "first\n").unwrap();
        std::fs::write(second.path.join("shared.txt"), "second\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(source.join("shared.txt")).unwrap(),
            "base\n"
        );
        assert_eq!(
            std::fs::read_to_string(first.path.join("shared.txt")).unwrap(),
            "first\n"
        );
        assert_eq!(
            std::fs::read_to_string(second.path.join("shared.txt")).unwrap(),
            "second\n"
        );
    }

    #[test]
    fn rejects_non_repository_and_nested_folder() {
        let temp = tempfile::tempdir().unwrap();
        assert!(create_chat_worktree(temp.path(), &temp.path().join("worktrees")).is_err());
        run(temp.path(), &["init", "-q"]);
        std::fs::write(temp.path().join("file"), "base").unwrap();
        run(temp.path(), &["add", "file"]);
        run(
            temp.path(),
            &[
                "-c",
                "user.name=Zest Test",
                "-c",
                "user.email=zest@example.invalid",
                "commit",
                "-qm",
                "base",
            ],
        );
        let nested = temp.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        assert!(create_chat_worktree(&nested, &temp.path().join("worktrees")).is_err());
    }
}
