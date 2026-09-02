//! Read-only project file browsing for the desktop workbench.
//!
//! The UI gets a shallow directory listing and bounded text previews. Every
//! requested path is canonicalized before it is read, so a symlink or `..`
//! component cannot turn the browser into an arbitrary filesystem reader.

use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

const MAX_DIRECTORY_ENTRIES: usize = 400;
const MAX_PREVIEW_BYTES: usize = 200_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceFileView {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub size: Option<u64>,
    pub modified_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceFileContent {
    pub path: String,
    pub content: String,
    pub truncated: bool,
    pub byte_count: u64,
}

pub(crate) fn list(root: &Path, relative: Option<&str>) -> Result<Vec<WorkspaceFileView>, String> {
    let root = canonical_root(root)?;
    let directory = resolve_path(&root, relative.unwrap_or_default())?;
    if contains_noise_directory(&relative_display(&root, &directory)) {
        return Err("that workspace path is hidden from the file browser".into());
    }
    if !directory.is_dir() {
        return Err("that workspace path is not a directory".into());
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(&directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        // Symlinks are intentionally omitted. Canonicalizing them would make a
        // directory tree appear safe while changing its target underneath us.
        if file_type.is_symlink() {
            continue;
        }
        if !file_type.is_dir() && !file_type.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if file_type.is_dir() && is_noise_directory(&name) {
            continue;
        }
        let metadata = entry.metadata().ok();
        let path = relative_display(&root, &entry.path());
        if zest_core::is_sensitive_path(&path) {
            continue;
        }
        entries.push(WorkspaceFileView {
            path,
            name,
            kind: if file_type.is_dir() {
                "directory".into()
            } else {
                "file".into()
            },
            size: metadata
                .as_ref()
                .filter(|_| file_type.is_file())
                .map(|m| m.len()),
            modified_at: metadata
                .and_then(|metadata| metadata.modified().ok())
                .and_then(unix_secs),
        });
    }

    entries.sort_by(|left, right| {
        let kind_order = (right.kind == "directory").cmp(&(left.kind == "directory"));
        if kind_order == Ordering::Equal {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
        } else {
            kind_order
        }
    });
    entries.truncate(MAX_DIRECTORY_ENTRIES);
    Ok(entries)
}

pub(crate) fn read(root: &Path, relative: &str) -> Result<WorkspaceFileContent, String> {
    let root = canonical_root(root)?;
    let path = resolve_path(&root, relative)?;
    let display = relative_display(&root, &path);
    if contains_noise_directory(&display) {
        return Err("that workspace path is hidden from the file browser".into());
    }
    if zest_core::is_sensitive_path(&display) {
        return Err("sensitive files are hidden from the workspace preview".into());
    }
    if !path.is_file() {
        return Err("that workspace path is not a file".into());
    }

    let size = fs::metadata(&path)
        .map_err(|error| error.to_string())?
        .len();
    let mut bytes = Vec::with_capacity(MAX_PREVIEW_BYTES + 1);
    File::open(&path)
        .map_err(|error| error.to_string())?
        .take((MAX_PREVIEW_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;

    let truncated = bytes.len() > MAX_PREVIEW_BYTES;
    if truncated {
        bytes.truncate(MAX_PREVIEW_BYTES);
    }
    let content = decode_preview(bytes, truncated)?;
    Ok(WorkspaceFileContent {
        path: display,
        content,
        truncated,
        byte_count: size,
    })
}

fn decode_preview(bytes: Vec<u8>, truncated: bool) -> Result<String, String> {
    match String::from_utf8(bytes) {
        Ok(content) => Ok(content),
        Err(error) => {
            let utf8_error = error.utf8_error();
            let incomplete_at_preview_boundary = truncated && utf8_error.error_len().is_none();
            if !incomplete_at_preview_boundary {
                return Err("binary files do not have a text preview".into());
            }

            let valid_up_to = utf8_error.valid_up_to();
            String::from_utf8(error.into_bytes()[..valid_up_to].to_vec())
                .map_err(|_| "binary files do not have a text preview".into())
        }
    }
}

fn canonical_root(root: &Path) -> Result<PathBuf, String> {
    let root = fs::canonicalize(root).map_err(|error| error.to_string())?;
    if !root.is_dir() {
        return Err("the workspace root is not a directory".into());
    }
    Ok(root)
}

fn resolve_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = Path::new(relative.trim());
    let mut normalized = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) => normalized.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("workspace paths must stay inside the project".into());
            }
        }
    }

    let candidate = root.join(normalized);
    let canonical = fs::canonicalize(&candidate).map_err(|error| error.to_string())?;
    if !canonical.starts_with(root) {
        return Err("workspace paths must stay inside the project".into());
    }
    Ok(canonical)
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn is_noise_directory(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        ".git" | ".zest" | "target" | "node_modules" | ".venv" | "dist"
    )
}

fn contains_noise_directory(path: &str) -> bool {
    path.split('/').any(is_noise_directory)
}

fn unix_secs(value: SystemTime) -> Option<u64> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_parent_paths() {
        let root = crate::ScratchDir::new("zest-files-");
        assert!(resolve_path(&root, "../outside").is_err());
    }

    #[test]
    fn lists_directories_before_files_and_omits_noise() {
        let root = crate::ScratchDir::new("zest-files-list-");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join(".env"), "SECRET=hidden").unwrap();
        fs::write(root.join(".env.example"), "SECRET=example").unwrap();
        fs::write(root.join("README.md"), "hello").unwrap();

        let entries = list(&root, None).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["src", ".env.example", "README.md"]
        );
        assert!(list(&root, Some("target")).is_err());
        assert!(read(&root, ".env").is_err());
    }

    #[test]
    fn truncates_large_text_previews() {
        let root = crate::ScratchDir::new("zest-files-read-");
        fs::write(root.join("large.txt"), "x".repeat(MAX_PREVIEW_BYTES + 10)).unwrap();

        let preview = read(&root, "large.txt").unwrap();
        assert!(preview.truncated);
        assert_eq!(preview.content.len(), MAX_PREVIEW_BYTES);
        assert_eq!(preview.byte_count, (MAX_PREVIEW_BYTES + 10) as u64);
    }

    #[test]
    fn truncates_at_a_utf8_boundary_instead_of_rejecting_text() {
        let root = crate::ScratchDir::new("zest-files-unicode-");
        let content = format!("{}é", "x".repeat(MAX_PREVIEW_BYTES - 1));
        fs::write(root.join("unicode.txt"), content).unwrap();

        let preview = read(&root, "unicode.txt").unwrap();
        assert!(preview.truncated);
        assert_eq!(preview.content.len(), MAX_PREVIEW_BYTES - 1);
        assert_eq!(preview.byte_count, (MAX_PREVIEW_BYTES + 1) as u64);
    }
}
