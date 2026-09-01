//! Ignore-aware workspace walking.
//!
//! Respects `.gitignore` (via the `ignore` crate) and always skips `.git`,
//! `.zest`, `target`, and `node_modules`. A walk never consults gitignore
//! files above the project root, and a folder that is not a git work tree
//! does not inherit the operator's global excludes — that combination is
//! what made a `/tmp` grep fixture look empty on Linux.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use super::project::ProjectRoot;
use super::sensitive::is_sensitive_path;

/// Directory names that are never entered, regardless of gitignore.
const HARD_SKIP_DIRS: &[&str] = &[".git", ".zest", "target", "node_modules"];

fn hard_skip_name(name: &str) -> bool {
    HARD_SKIP_DIRS.contains(&name)
}

/// Shared ignore rules for project-scoped walks (grep, glob, list).
///
/// `parents(false)` keeps a gitignore sitting in `/tmp` or `$HOME` from hiding
/// files inside a project that merely lives underneath. `git_global` stays off
/// until the project itself is a git work tree, so a unit-test scratch dir is
/// not filtered by `~/.config/git/ignore`.
pub(crate) fn configure_walk_builder(builder: &mut WalkBuilder, project_root: &Path) {
    let git_repo = project_root.join(".git").exists();
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(git_repo)
        .git_exclude(git_repo)
        .require_git(false)
        .parents(false)
        .follow_links(false)
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            if entry.depth() > 0 && hard_skip_name(&name) {
                return false;
            }
            true
        });
    let gitignore = project_root.join(".gitignore");
    if gitignore.is_file() {
        let _ = builder.add_ignore(&gitignore);
    }
}

/// Walk files under `start` (must already be confined under `root`), handing
/// each canonical, non-sensitive path to `visit`.
///
/// `visit` returns `false` to stop the walk.
///
/// Streaming rather than returning a `Vec` because the caller bounds its own
/// results: collecting the whole tree first meant a caller's cap saved nothing,
/// since on a large repository the walk *is* the cost and the matching is free.
pub fn walk_files(root: &ProjectRoot, start: &Path, mut visit: impl FnMut(PathBuf) -> bool) {
    let mut builder = WalkBuilder::new(start);
    configure_walk_builder(&mut builder, root.as_path());

    for entry in builder.build().flatten() {
        let path = entry.path();
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let Ok(resolved) = root.confine(path) else {
            continue;
        };
        if !resolved.is_file() {
            continue;
        }
        let rel = root.relativize(&resolved);
        if is_sensitive_path(&rel) {
            continue;
        }
        if !visit(resolved) {
            return;
        }
    }
}

/// Immediate children of `dir` (confined), skip hard-skip dirs and sensitive files.
/// Applies gitignore for entries under the project.
pub fn list_children(root: &ProjectRoot, dir: &Path) -> Result<Vec<ListedEntry>, String> {
    let mut builder = WalkBuilder::new(dir);
    configure_walk_builder(&mut builder, root.as_path());
    builder.max_depth(Some(1));

    let mut entries = Vec::new();
    for entry in builder.build().flatten() {
        if entry.depth() == 0 {
            continue;
        }
        let path = entry.path();
        let name = entry
            .file_name()
            .to_str()
            .ok_or_else(|| format!("non-utf8 directory entry under {}", dir.display()))?
            .to_string();

        if hard_skip_name(&name) {
            continue;
        }

        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            // Confine directories when they exist; skip symlink escapes.
            if root.confine(path).is_err() {
                continue;
            }
            entries.push(ListedEntry { name, is_dir: true });
            continue;
        }

        let Ok(resolved) = root.confine(path) else {
            continue;
        };
        let rel = if dir == root.as_path() {
            name.clone()
        } else {
            format!("{}/{}", root.relativize(dir), name).replace('\\', "/")
        };
        if is_sensitive_path(&rel) || is_sensitive_path(&name) {
            continue;
        }
        if resolved.is_file() || path.is_symlink() {
            entries.push(ListedEntry {
                name,
                is_dir: false,
            });
        }
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

#[derive(Debug, Clone)]
pub struct ListedEntry {
    pub name: String,
    pub is_dir: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zest-walk-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn skips_hard_dirs_and_sensitive() {
        let dir = scratch("skip");
        fs::create_dir_all(dir.join("target/debug")).unwrap();
        fs::write(dir.join("target/debug/x"), "x").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(dir.join(".env"), "SECRET=1").unwrap();
        fs::write(dir.join(".env.example"), "SECRET=").unwrap();
        fs::write(dir.join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(dir.join("ignored.txt"), "nope").unwrap();
        fs::write(dir.join("keep.txt"), "yes").unwrap();

        let root = ProjectRoot::new(&dir).unwrap();
        let mut rels: Vec<String> = Vec::new();
        walk_files(&root, root.as_path(), |path| {
            rels.push(root.relativize(&path));
            true
        });

        assert!(rels.iter().any(|r| r == "src/main.rs"), "{rels:?}");
        assert!(rels.iter().any(|r| r == "keep.txt"), "{rels:?}");
        assert!(rels.iter().any(|r| r == ".env.example"), "{rels:?}");
        assert!(!rels.iter().any(|r| r.contains("target")), "{rels:?}");
        assert!(!rels.iter().any(|r| r == ".env"), "{rels:?}");
        assert!(!rels.iter().any(|r| r == "ignored.txt"), "{rels:?}");
    }

    #[test]
    fn list_children_omits_sensitive() {
        let dir = scratch("list");
        fs::write(dir.join(".env"), "x").unwrap();
        fs::write(dir.join("ok.txt"), "x").unwrap();
        fs::create_dir(dir.join("node_modules")).unwrap();

        let root = ProjectRoot::new(&dir).unwrap();
        let kids = list_children(&root, root.as_path()).unwrap();
        let names: Vec<&str> = kids.iter().map(|k| k.name.as_str()).collect();
        assert!(names.contains(&"ok.txt"), "{names:?}");
        assert!(!names.contains(&".env"), "{names:?}");
        assert!(!names.contains(&"node_modules"), "{names:?}");
    }
}
