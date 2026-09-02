//! Project-rooted path confinement shared by tools.
//!
//! Every path that reaches the filesystem is canonicalized and checked against
//! the project root, which closes `..`, absolute paths, and symlink escapes.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// Canonical project root that tools resolve paths against.
#[derive(Clone, Debug)]
pub struct ProjectRoot {
    root: PathBuf,
}

impl ProjectRoot {
    pub fn new(root: impl AsRef<Path>) -> std::io::Result<Self> {
        Ok(Self {
            root: std::fs::canonicalize(root)?,
        })
    }

    pub fn as_path(&self) -> &Path {
        &self.root
    }

    /// Resolve `raw` (relative to the project root) to an existing path that
    /// stays inside the root after symlink resolution.
    pub fn resolve(&self, raw: &str) -> Result<PathBuf, String> {
        let candidate = if raw.is_empty() || raw == "." {
            self.root.clone()
        } else {
            self.root.join(raw)
        };

        // canonicalize resolves `..` and symlinks; it also requires the path to
        // exist, so a missing path is reported here rather than later.
        let resolved = std::fs::canonicalize(&candidate)
            .map_err(|e| format!("cannot resolve `{raw}`: {e}"))?;

        if !resolved.starts_with(&self.root) {
            return Err(format!("`{raw}` resolves outside the project root"));
        }
        Ok(resolved)
    }

    /// Canonicalize an already-joined path and reject escapes. Used while
    /// walking so symlink targets outside the tree are never opened.
    pub fn confine(&self, path: &Path) -> Result<PathBuf, String> {
        let resolved = std::fs::canonicalize(path)
            .map_err(|e| format!("cannot resolve `{}`: {e}", path.display()))?;
        if !resolved.starts_with(&self.root) {
            return Err(format!(
                "`{}` resolves outside the project root",
                path.display()
            ));
        }
        Ok(resolved)
    }

    /// Path relative to the project root, using forward slashes for stable
    /// tool output across platforms.
    pub fn relativize(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
    }

    /// Resolve a relative path for create/overwrite. Unlike [`resolve`], the
    /// target file need not exist yet; the nearest existing ancestor is
    /// canonicalized so symlink escapes are still rejected.
    pub fn resolve_for_write(&self, raw: &str) -> Result<PathBuf, String> {
        let relative = normalize_relative(raw)?;
        let candidate = self.root.join(&relative);

        if let Ok(resolved) = std::fs::canonicalize(&candidate) {
            if !resolved.starts_with(&self.root) {
                return Err(format!("`{raw}` resolves outside the project root"));
            }
            if resolved.is_dir() {
                return Err(format!("`{raw}` is a directory, not a file"));
            }
            return Ok(resolved);
        }

        let mut suffix: Vec<OsString> = Vec::new();
        let mut cursor = candidate.as_path();
        loop {
            if let Some(name) = cursor.file_name() {
                suffix.push(name.to_os_string());
            }
            let parent = cursor
                .parent()
                .ok_or_else(|| format!("cannot resolve parent for `{raw}`"))?;
            if let Ok(canon_parent) = std::fs::canonicalize(parent) {
                if !canon_parent.starts_with(&self.root) {
                    return Err(format!("`{raw}` resolves outside the project root"));
                }
                let mut path = canon_parent;
                for part in suffix.iter().rev() {
                    path.push(part);
                }
                if !path.starts_with(&self.root) {
                    return Err(format!("`{raw}` resolves outside the project root"));
                }
                return Ok(path);
            }
            if parent == self.root || !parent.starts_with(&self.root) {
                break;
            }
            cursor = parent;
        }

        Err(format!(
            "cannot resolve `{raw}`: no existing parent under project root"
        ))
    }
}

/// Strip `.` / `..` from a project-relative path without touching the filesystem.
fn normalize_relative(raw: &str) -> Result<PathBuf, String> {
    let path = Path::new(raw);
    if path.is_absolute() {
        return Err("path must be relative to the project root".into());
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return Err(format!("`{raw}` escapes the project root"));
                }
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err("path must be relative to the project root".into());
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err("path must name a file inside the project".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> crate::fsutil::ScratchDir {
        crate::fsutil::ScratchDir::new(&format!("zest-project-{name}-"))
    }

    #[test]
    fn resolve_accepts_paths_under_root() {
        let dir = scratch("under");
        std::fs::write(dir.join("a.txt"), "hi").unwrap();
        let root = ProjectRoot::new(&dir).unwrap();
        let resolved = root.resolve("a.txt").unwrap();
        assert!(resolved.ends_with("a.txt"));
    }

    #[test]
    fn resolve_rejects_parent_escape() {
        let dir = scratch("escape");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        // A sibling outside the project root.
        let outside = dir
            .parent()
            .unwrap()
            .join("zest-project-escape-outside.txt");
        std::fs::write(&outside, "secret").unwrap();

        let root = ProjectRoot::new(&dir).unwrap();
        let err = root
            .resolve("../zest-project-escape-outside.txt")
            .unwrap_err();
        assert!(
            err.contains("outside the project root") || err.contains("cannot resolve"),
            "{err}"
        );
        let _ = std::fs::remove_file(outside);
    }

    #[test]
    fn resolve_dot_is_root() {
        let dir = scratch("dot");
        let root = ProjectRoot::new(&dir).unwrap();
        assert_eq!(root.resolve(".").unwrap(), root.as_path());
        assert_eq!(root.resolve("").unwrap(), root.as_path());
    }

    #[test]
    fn resolve_for_write_creates_under_root() {
        let dir = scratch("write-new");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let root = ProjectRoot::new(&dir).unwrap();
        let path = root.resolve_for_write("src/new.rs").unwrap();
        assert!(path.ends_with("new.rs"));
        assert!(path.starts_with(root.as_path()));
    }

    #[test]
    fn resolve_for_write_rejects_escape() {
        let dir = scratch("write-escape");
        let root = ProjectRoot::new(&dir).unwrap();
        let err = root.resolve_for_write("../outside.txt").unwrap_err();
        assert!(err.contains("escapes") || err.contains("outside"), "{err}");
    }
}
