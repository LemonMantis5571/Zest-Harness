use std::ffi::OsStr;
use std::ops::Deref;
use std::path::Path;

/// Test scratch directory that deletes itself on drop.
pub(crate) struct ScratchDir {
    inner: tempfile::TempDir,
}

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

impl Deref for ScratchDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        self.inner.path()
    }
}

impl AsRef<Path> for ScratchDir {
    fn as_ref(&self) -> &Path {
        self.inner.path()
    }
}

impl AsRef<OsStr> for ScratchDir {
    fn as_ref(&self) -> &OsStr {
        self.inner.path().as_os_str()
    }
}
