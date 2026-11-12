use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Exclusive coordinator lock for one project root.
///
/// The file stays open for the lifetime of this value. Desktop and `zest serve`
/// share `.zest/delegations/coordinator.lock`, so a second process fails instead
/// of dispatching or applying the same card twice.
pub struct CoordinatorLock {
    file: File,
    path: PathBuf,
}

impl Drop for CoordinatorLock {
    /// Unlock before the file closes. Closing alone releases the lock only once
    /// every descriptor sharing the open file is closed, and a process another
    /// thread is forking (git, a worker) holds a copy until it execs; the lock
    /// then outlived this value, and the next acquire here failed as "another
    /// coordinator already owns this project".
    fn drop(&mut self) {
        unlock(&self.file);
    }
}

/// Start of the error returned when another process holds the project lock.
pub const LOCK_HELD_MESSAGE: &str = "another coordinator already owns this project";

/// True when `error` came from [`CoordinatorLock::acquire`] finding the lock held.
pub fn is_lock_held_error(error: &str) -> bool {
    error.starts_with(LOCK_HELD_MESSAGE)
}

pub fn lock_path(root: &Path) -> PathBuf {
    root.join(".zest")
        .join("delegations")
        .join("coordinator.lock")
}

impl CoordinatorLock {
    pub fn acquire(root: &Path) -> Result<Self, String> {
        let dir = root.join(".zest").join("delegations");
        std::fs::create_dir_all(&dir).map_err(|error| {
            format!(
                "could not create delegation store at {}: {error}",
                dir.display()
            )
        })?;
        let path = lock_path(root);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| format!("could not open {}: {error}", path.display()))?;
        match try_lock_exclusive(&file) {
            Ok(true) => Ok(Self { file, path }),
            Ok(false) => Err(format!("{LOCK_HELD_MESSAGE} (lock {})", path.display())),
            Err(error) => Err(format!("could not lock {}: {error}", path.display())),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(unix)]
fn try_lock_exclusive(file: &File) -> io::Result<bool> {
    use std::os::unix::io::AsRawFd;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    if err.kind() == io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(err)
    }
}

#[cfg(unix)]
fn unlock(file: &File) {
    use std::os::unix::io::AsRawFd;
    // Best effort: closing the file still releases it once every copy is gone.
    let _ = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
}

#[cfg(windows)]
fn unlock(file: &File) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
    use windows_sys::Win32::System::IO::OVERLAPPED;

    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    // Best effort: closing the file still releases it once every copy is gone.
    let _ = unsafe { UnlockFileEx(file.as_raw_handle() as HANDLE, 0, 1, 0, &mut overlapped) };
}

#[cfg(windows)]
fn try_lock_exclusive(file: &File) -> io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;

    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        LockFileEx(
            file.as_raw_handle() as HANDLE,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut overlapped,
        )
    };
    if ok != 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    // ERROR_LOCK_VIOLATION
    if err.raw_os_error() == Some(33) {
        Ok(false)
    } else {
        Err(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_released_lock_can_be_taken_while_another_descriptor_is_open() {
        // A process forked by another thread (git, a worker) holds a copy of
        // every descriptor until it execs. Closing ours then did not release
        // the lock, and the next acquire in this process failed as "another
        // coordinator". A cloned handle stands in for that child's copy.
        let temp = tempfile::tempdir().unwrap();
        let first = CoordinatorLock::acquire(temp.path()).unwrap();
        let child_copy = first.file.try_clone().unwrap();
        drop(first);
        let again = CoordinatorLock::acquire(temp.path());
        assert!(again.is_ok(), "{:?}", again.err());
        drop(child_copy);
    }
}
