//! Persistent Git worktrees for independent coding chats.
//!
//! Delegated workers use disposable worktrees and copy the caller's dirty
//! snapshot. A chat worktree is different: it starts at a committed HEAD and
//! stays on disk until the user deliberately removes it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::thread::new_id;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatWorktree {
    pub path: PathBuf,
    pub branch: String,
    /// The commit the checkout started from, kept so its status can still be
    /// compared when the source project has no branch to compare against.
    pub base_commit: String,
}

/// Git counts shown beside a chat worktree in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChatWorktreeStatus {
    /// Commits on the chat's HEAD that the source branch does not have. `0`
    /// when there is nothing to compare against.
    pub ahead: u32,
    /// Commits on the source branch that the chat's HEAD does not have.
    pub behind: u32,
    /// Entries in `git status --porcelain`: tracked changes plus untracked
    /// files, where a new untracked directory counts once.
    pub dirty: u32,
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

/// Run a read-only Git query, or `None` if Git cannot answer. It never takes
/// the index lock, so refreshing the sidebar cannot make the agent's own
/// `git add` fail, and on Windows it does not flash a console window.
fn query(root: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn is_object_id(candidate: &str) -> bool {
    matches!(candidate.len(), 40 | 64) && candidate.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The commit to count ahead/behind against: the tip of the branch the source
/// project is on, else the commit the worktree was created from.
fn comparison_point(source: &Path, base_commit: Option<&str>) -> Option<String> {
    let source_tip = query(source, &["symbolic-ref", "--quiet", "HEAD"])
        .and_then(|_| query(source, &["rev-parse", "--verify", "--quiet", "HEAD"]))
        .map(|tip| tip.trim().to_string());
    source_tip
        .or_else(|| base_commit.map(str::to_string))
        .filter(|point| is_object_id(point))
}

fn parse_left_right(counts: &str) -> Option<(u32, u32)> {
    let mut fields = counts.split_whitespace();
    let left = fields.next()?.parse().ok()?;
    let right = fields.next()?.parse().ok()?;
    Some((left, right))
}

/// Read a chat checkout's ahead/behind/dirty counts. `source` is the project
/// the chat was started from. A missing source, a detached source HEAD or an
/// unknown base only drop the ahead/behind counts; `None` means Git could not
/// read the checkout at all, so the caller should show nothing.
pub fn chat_worktree_status(
    worktree: &Path,
    source: &Path,
    base_commit: Option<&str>,
) -> Option<ChatWorktreeStatus> {
    let porcelain = query(
        worktree,
        &["status", "--porcelain", "--untracked-files=normal"],
    )?;
    let dirty = porcelain
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    let (behind, ahead) = comparison_point(source, base_commit)
        .and_then(|point| {
            query(
                worktree,
                &[
                    "rev-list",
                    "--left-right",
                    "--count",
                    &format!("{point}...HEAD"),
                    "--",
                ],
            )
        })
        .and_then(|counts| parse_left_right(&counts))
        .unwrap_or((0, 0));
    Some(ChatWorktreeStatus {
        ahead,
        behind,
        dirty: u32::try_from(dirty).unwrap_or(u32::MAX),
    })
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
    Ok(ChatWorktree {
        path,
        branch,
        base_commit: head,
    })
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

    fn commit_all(root: &Path, message: &str) {
        run(root, &["add", "-A"]);
        run(
            root,
            &[
                "-c",
                "user.name=Zest Test",
                "-c",
                "user.email=zest@example.invalid",
                "commit",
                "-qm",
                message,
            ],
        );
    }

    fn source_repository(temp: &Path) -> PathBuf {
        let source = temp.join("source");
        std::fs::create_dir(&source).unwrap();
        run(&source, &["init", "-q"]);
        std::fs::write(source.join("shared.txt"), "base\n").unwrap();
        commit_all(&source, "base");
        source
    }

    #[test]
    fn status_counts_commits_on_each_side_and_uncommitted_files() {
        let temp = tempfile::tempdir().unwrap();
        let source = source_repository(temp.path());
        let storage = temp.path().join("worktrees");
        let chat = create_chat_worktree(&source, &storage).unwrap();
        let idle = create_chat_worktree(&source, &storage).unwrap();

        for name in ["one.txt", "two.txt"] {
            std::fs::write(chat.path.join(name), name).unwrap();
            commit_all(&chat.path, name);
        }
        std::fs::write(source.join("elsewhere.txt"), "source moved on\n").unwrap();
        commit_all(&source, "source moved on");

        std::fs::write(chat.path.join("shared.txt"), "edited\n").unwrap();
        std::fs::write(chat.path.join("staged.txt"), "staged\n").unwrap();
        run(&chat.path, &["add", "staged.txt"]);
        std::fs::write(chat.path.join("untracked.txt"), "new\n").unwrap();
        std::fs::create_dir(chat.path.join("new-dir")).unwrap();
        std::fs::write(chat.path.join("new-dir").join("a.txt"), "a").unwrap();
        std::fs::write(chat.path.join("new-dir").join("b.txt"), "b").unwrap();

        // Modified, staged, untracked file and one untracked directory.
        let expected = ChatWorktreeStatus {
            ahead: 2,
            behind: 1,
            dirty: 4,
        };
        assert_eq!(
            chat_worktree_status(&chat.path, &source, Some(&chat.base_commit)),
            Some(expected)
        );
        assert_eq!(
            chat_worktree_status(&idle.path, &source, Some(&idle.base_commit)),
            Some(ChatWorktreeStatus {
                ahead: 0,
                behind: 1,
                dirty: 0
            })
        );

        // A detached source has no branch, so the creation commit stands in.
        run(&source, &["checkout", "-q", "--detach"]);
        assert_eq!(
            chat_worktree_status(&chat.path, &source, Some(&chat.base_commit)),
            Some(ChatWorktreeStatus {
                behind: 0,
                ..expected
            })
        );
        // With no base either, only the dirty count remains. A malformed base
        // is ignored rather than handed to Git as an option.
        for base in [None, Some("--output=leak"), Some("not-a-commit")] {
            assert_eq!(
                chat_worktree_status(&chat.path, &source, base),
                Some(ChatWorktreeStatus {
                    ahead: 0,
                    behind: 0,
                    dirty: 4
                })
            );
        }
    }

    #[test]
    fn status_is_absent_when_git_cannot_read_the_checkout() {
        let temp = tempfile::tempdir().unwrap();
        let source = source_repository(temp.path());
        let chat = create_chat_worktree(&source, &temp.path().join("worktrees")).unwrap();
        assert!(chat_worktree_status(&chat.path, &source, Some(&chat.base_commit)).is_some());

        assert_eq!(
            chat_worktree_status(&temp.path().join("absent"), &source, None),
            None
        );
        // Removing the source takes the linked checkout's Git data with it.
        std::fs::remove_dir_all(&source).unwrap();
        assert_eq!(
            chat_worktree_status(&chat.path, &source, Some(&chat.base_commit)),
            None
        );
    }
}
