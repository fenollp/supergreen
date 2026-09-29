use std::io;

use camino::{Utf8Path, Utf8PathBuf};
use futures::future::LocalBoxFuture;

/// Filesystem operations.
///
/// Returns `io::Result` so `Into` and `io::ErrorKind` gets done above.
pub(crate) trait Fs: Send + Sync {
    /// Reads the entire contents of a file into a string
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String>;

    /// Writes a slice as the entire contents of a file
    fn write(&self, path: &Utf8Path, data: &str) -> io::Result<()>;

    /// Write through a temporary file then `rename`, so readers never see a partial file
    fn write_atomic(&self, path: &Utf8Path, data: &str) -> io::Result<()>;

    /// Writes to a file in append mode
    fn append(&self, path: &Utf8Path, data: &str) -> io::Result<()>;

    /// Copies the contents of one file to another
    fn copy(&self, from: &Utf8Path, to: &Utf8Path) -> io::Result<()>;

    /// Equivalent to `mkdir -p`
    fn create_dir_all(&self, path: &Utf8Path) -> io::Result<()>;

    /// Removes a file or fails if it does not exist
    fn remove_file(&self, path: &Utf8Path) -> io::Result<()>;

    /// Returns false on permission errors and through broken symlinks
    #[must_use]
    fn exists(&self, path: &Utf8Path) -> bool;

    /// Returns false on permission errors or if it does not exist
    #[must_use]
    fn is_dir(&self, path: &Utf8Path) -> bool;

    /// File names of `path`'s entries, in unspecified order.
    fn read_dir(&self, path: &Utf8Path) -> io::Result<Vec<Utf8PathBuf>>;

    /// Digests the contents of a file
    #[must_use]
    fn sha256<'a>(&'a self, path: &'a Utf8Path) -> LocalBoxFuture<'a, io::Result<String>>;
}
