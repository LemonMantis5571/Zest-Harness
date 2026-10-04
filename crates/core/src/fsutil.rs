//! Centralized atomic persistence helpers.
//!
//! All durable project/user state (threads, usage, system prompts, preferences)
//! should go through [`atomic_write`]: unique temp file → flush/sync → Windows
//! `MoveFileExW(REPLACE_EXISTING)` (or POSIX rename) without deleting a valid
//! destination first.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Write `data` via a unique temp file in the target's parent, flush + sync,
/// then atomically replace the destination.
pub fn atomic_write(target: &Path, data: &[u8]) -> std::io::Result<()> {
    let parent = target.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "target has no parent directory",
        )
    })?;
    std::fs::create_dir_all(parent)?;

    let temp = unique_temp_path(parent, target)?;
    let write_result = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(data)?;
        file.flush()?;
        file.sync_data()?;
        Ok(())
    })();

    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }

    match atomic_replace(&temp, target) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(e)
        }
    }
}

/// Convenience: serialize `value` as pretty JSON and atomically replace `target`.
///
/// Pretty because most of what Zest writes is meant to be opened and read — a
/// ledger, a config, a thread. For files only Zest ever reads, prefer
/// [`atomic_write_json_compact`].
pub fn atomic_write_json<T: serde::Serialize>(target: &Path, value: &T) -> std::io::Result<()> {
    let body = serde_json::to_vec_pretty(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    atomic_write(target, &body)
}

/// Create a project root and persist newly created directory entries on Unix.
/// Stop at the first existing directory, so existing projects do not require
/// read access to ancestors above them. Call before accepting receipt-backed
/// commands in a newly initialized project.
pub fn create_dir_all_durable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let absolute = std::env::current_dir()?.join(path);
        let mut missing = Vec::new();
        let mut existing = absolute.as_path();
        loop {
            match std::fs::metadata(existing) {
                Ok(metadata) if metadata.is_dir() => break,
                Ok(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotADirectory,
                        "project ancestor is not a directory",
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    missing.push(existing.to_path_buf());
                    existing = existing.parent().ok_or(error)?;
                }
                Err(error) => return Err(error),
            }
        }
        std::fs::create_dir_all(&absolute)?;
        if !missing.is_empty() {
            for directory in missing
                .iter()
                .map(PathBuf::as_path)
                .chain(std::iter::once(existing))
            {
                std::fs::File::open(directory)?.sync_all()?;
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(path)
}

/// Persist a receipt before a side effect may run. On Unix, syncing only the
/// file is insufficient: the rename and newly created parent directories must
/// reach disk too. `durability_root` is the existing project directory; ancestors
/// above it need not be readable. Directory sync failures reach the caller.
pub fn atomic_write_json_durable<T: serde::Serialize>(
    target: &Path,
    value: &T,
    durability_root: &Path,
) -> std::io::Result<()> {
    let parent = target
        .parent()
        .filter(|parent| parent.starts_with(durability_root));
    if parent.is_none() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "durable target is outside the project directory",
        ));
    }
    atomic_write_json(target, value)?;
    #[cfg(unix)]
    for parent in target
        .ancestors()
        .skip(1)
        .take_while(|parent| parent.starts_with(durability_root))
    {
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

/// The same, without the indentation.
///
/// For machine-only files large enough that the whitespace is the file. The
/// transcript scan cache holds ~50,000 rows; pretty-printing it costs 6 MB of
/// indentation for a document no person will ever open.
pub fn atomic_write_json_compact<T: serde::Serialize>(
    target: &Path,
    value: &T,
) -> std::io::Result<()> {
    let body = serde_json::to_vec(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    atomic_write(target, &body)
}

fn unique_temp_path(parent: &Path, target: &Path) -> std::io::Result<PathBuf> {
    let stem = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("write");
    let pid = std::process::id();
    for attempt in 0..64u32 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let name = format!(".zest-{stem}-{pid}-{nanos}-{attempt}.tmp");
        let candidate = parent.join(name);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique temp file name",
    ))
}

fn atomic_replace(temp: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        replace_windows(temp, target)
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(temp, target)
    }
}

#[cfg(windows)]
fn replace_windows(temp: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    extern "system" {
        fn MoveFileExW(
            lp_existing_file_name: *const u16,
            lp_new_file_name: *const u16,
            dw_flags: u32,
        ) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    let from: Vec<u16> = temp
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let to: Vec<u16> = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let ok = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Strip the Windows `\\?\` extended-path prefix for anything a person reads.
///
/// `canonicalize()` on Windows returns paths like `\\?\D:\Code\zest`. That form
/// is correct for filesystem APIs and looks broken everywhere else, so it must
/// not reach UI copy or an error message.
pub fn display_path(path: &Path) -> String {
    display_path_str(&path.display().to_string())
}

pub fn display_path_str(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        raw.to_string()
    }
}

/// Auto-deleting test scratch dir. Keep the value alive for the whole test.
#[cfg(test)]
pub(crate) struct ScratchDir {
    inner: tempfile::TempDir,
}

#[cfg(test)]
impl ScratchDir {
    pub(crate) fn new(prefix: &str) -> Self {
        Self {
            inner: tempfile::Builder::new()
                .prefix(prefix)
                .tempdir()
                .expect("scratch dir"),
        }
    }
}

#[cfg(test)]
impl std::ops::Deref for ScratchDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        self.inner.path()
    }
}

#[cfg(test)]
impl AsRef<Path> for ScratchDir {
    fn as_ref(&self) -> &Path {
        self.inner.path()
    }
}

#[cfg(test)]
impl AsRef<std::ffi::OsStr> for ScratchDir {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.inner.path().as_os_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> ScratchDir {
        ScratchDir::new(&format!("zest-fsutil-{name}-"))
    }

    #[test]
    fn display_path_strips_windows_extended_prefix() {
        assert_eq!(display_path_str(r"\\?\D:\Code\zest"), r"D:\Code\zest");
        assert_eq!(
            display_path_str(r"\\?\UNC\server\share\repo"),
            r"\\server\share\repo"
        );
        assert_eq!(display_path_str(r"D:\Code\zest"), r"D:\Code\zest");
        assert_eq!(display_path_str("/home/u/code"), "/home/u/code");
    }

    #[test]
    fn atomic_write_creates_and_replaces() {
        let dir = scratch("replace");
        let path = dir.join("state.json");
        atomic_write(&path, b"one").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one");
        atomic_write(&path, b"two").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
        // No leftover temps in the parent.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn durable_json_write_creates_nested_directories_and_replaces_the_record() {
        let dir = scratch("durable-receipt");
        let path = dir.join("new-state").join("receipts").join("command.json");
        atomic_write_json_durable(&path, &serde_json::json!({"state":"pending"}), &dir).unwrap();
        let first: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(first, serde_json::json!({"state":"pending"}));
        atomic_write_json_durable(&path, &serde_json::json!({"state":"completed"}), &dir).unwrap();
        let completed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(completed, serde_json::json!({"state":"completed"}));
    }

    #[test]
    fn durable_directory_creation_preserves_existing_project_contents() {
        let dir = scratch("durable-directory");
        let project = dir.join("new-parent").join("project");
        create_dir_all_durable(&project).unwrap();
        assert!(project.is_dir());
        let marker = project.join("keep.txt");
        std::fs::write(&marker, "keep me").unwrap();
        create_dir_all_durable(&project).unwrap();
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "keep me");
        assert!(create_dir_all_durable(&project.join("keep.txt").join("invalid")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn durable_write_does_not_require_reading_ancestors_above_the_project() {
        use std::os::unix::fs::PermissionsExt;
        let outer = scratch("execute-only-ancestor");
        let project = outer.join("project");
        std::fs::create_dir(&project).unwrap();
        let original_permissions = std::fs::metadata(&outer).unwrap().permissions();
        std::fs::set_permissions(&outer, std::fs::Permissions::from_mode(0o300)).unwrap();
        let target = project.join(".zest").join("receipts").join("command.json");
        let result = atomic_write_json_durable(&target, &serde_json::json!("pending"), &project);
        std::fs::set_permissions(&outer, original_permissions).unwrap();
        result.unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(target).unwrap()).unwrap();
        assert_eq!(saved, serde_json::json!("pending"));
    }

    #[test]
    fn durable_write_refuses_a_target_outside_its_project() {
        let dir = scratch("durability-boundary");
        let project = dir.join("project");
        let inside = project.join("receipt.json");
        atomic_write_json_durable(&inside, &serde_json::json!("pending"), &project).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(inside).unwrap()).unwrap(),
            serde_json::json!("pending")
        );
        let path = dir.join("outside.json");
        let result = atomic_write_json_durable(&path, &serde_json::json!("pending"), &project);
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
        assert!(!path.exists());
    }
}
