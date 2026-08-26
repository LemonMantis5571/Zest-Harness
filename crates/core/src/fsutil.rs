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
        file.sync_all()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zest-fsutil-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
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
}
