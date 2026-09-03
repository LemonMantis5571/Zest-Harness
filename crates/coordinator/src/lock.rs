use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Exclusive coordinator lock for one project root.
///
/// The file stays open for the lifetime of this value. Desktop and `zest serve`
/// share `.zest/delegations/coordinator.lock`, so a second process fails instead
/// of dispatching or applying the same card twice.
pub struct CoordinatorLock {
    _file: File,
    path: PathBuf,
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
            Ok(true) => Ok(Self { _file: file, path }),
            Ok(false) => Err(format!(
                "another coordinator already owns this project (lock {})",
                path.display()
            )),
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
