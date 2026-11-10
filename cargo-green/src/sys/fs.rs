use std::{io, time::SystemTime};

use camino::{Utf8Path, Utf8PathBuf};
use futures::future::BoxFuture;

/// Only owned by $USER
pub(crate) const ONLY_OURS_MODE: u32 = 0o600;

/// Sockets live there: for our eyes only
pub(crate) const VAR_RUN_MODE: u32 = 0o700;

/// Our tarballs (results, ...)
pub(crate) const TAR_MODE: u32 = 0o644;

/// Tar archives are made of 512-byte blocks.
pub(crate) const TAR_BLOCK: u64 = 512;

/// What's being written through a temporary file: it appears, whole, once `commit`ted.
pub(crate) trait AtomicWriter: io::Write + Send {
    /// Makes the file appear. Dropping the writer instead leaves nothing behind.
    fn commit(self: Box<Self>) -> io::Result<()>;
}

/// A file that has no name: gone once dropped, whatever happens.
pub(crate) trait Spool: io::Read + io::Write + io::Seek + Send {}

impl<T: io::Read + io::Write + io::Seek + Send> Spool for T {}

/// What a path takes on disk, symlinks not followed.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) struct Allocated {
    /// Bytes allocated to the path alone, not to what's under it
    pub(crate) bytes: u64,
    pub(crate) is_dir: bool,
    /// Which file this is, when it has several names: (device, inode)
    pub(crate) hard_linked: Option<(u64, u64)>,
}

/// Filesystem operations.
///
/// Returns `io::Result` so `Into` and `io::ErrorKind` gets done above.
pub(crate) trait Fs: Send + Sync {
    /// Opens a file to read it as it goes
    fn open(&self, path: &Utf8Path) -> io::Result<Box<dyn io::Read + Send>>;

    /// Reads the entire contents of a file into a string
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String>;

    /// Writes a slice as the entire contents of a file
    fn write(&self, path: &Utf8Path, data: &str) -> io::Result<()>;

    /// Write through a temporary file then `rename`, so readers never see a partial file
    fn write_atomic(&self, path: &Utf8Path, data: &str) -> io::Result<()>;

    /// Same as `write_atomic`, for bytes and creating the file with `mode`
    fn write_atomic_bytes(&self, path: &Utf8Path, data: &[u8], mode: u32) -> io::Result<()>;

    /// Same as `write_atomic_bytes`, for contents that aren't all there yet
    fn atomic_writer(&self, path: &Utf8Path, mode: u32) -> io::Result<Box<dyn AtomicWriter>>;

    /// Creates a [`Spool`] (doesn't get to stay)
    fn spool(&self, path: &Utf8Path) -> io::Result<Box<dyn Spool>>;

    /// Writes to a file in append mode
    fn append(&self, path: &Utf8Path, data: &str) -> io::Result<()>;

    /// Copies the contents of one file to another
    fn copy(&self, from: &Utf8Path, to: &Utf8Path) -> io::Result<()>;

    /// Equivalent to `mkdir -p`
    fn create_dir_all(&self, path: &Utf8Path) -> io::Result<()>;

    /// Equivalent to `mkdir -p -m <mode>`
    fn create_dir_all_mode(&self, path: &Utf8Path, mode: u32) -> io::Result<()>;

    /// Equivalent to `ln -sf <original> <link>`
    fn symlink_file(&self, original: &Utf8Path, link: &Utf8Path) -> io::Result<()>;

    /// Equivalent to `ln <original> <link>`: both names lead to the same file
    /// Fails if `link` exists
    #[cfg_attr(not(test), expect(dead_code))]
    fn hard_link(&self, original: &Utf8Path, link: &Utf8Path) -> io::Result<()>;

    /// Equivalent to `ln -s <original> <link>`, for a directory
    /// Fails if `link` exists
    fn symlink_dir(&self, original: &Utf8Path, link: &Utf8Path) -> io::Result<()>;

    /// Removes a symlink to a directory
    /// Fails if `link` is anything else
    fn remove_symlink_dir(&self, link: &Utf8Path) -> io::Result<()>;

    /// What a symlink points to
    fn read_link(&self, link: &Utf8Path) -> io::Result<Utf8PathBuf>;

    /// Absolute path, with all symlinks followed
    /// Fails if it doesn't lead anywhere
    fn canonicalize(&self, path: &Utf8Path) -> io::Result<Utf8PathBuf>;

    /// When a file or dir was last modified (symlinks followed)
    fn modified(&self, path: &Utf8Path) -> io::Result<SystemTime>;

    /// Permission bits of a file, dir or symlink (not followed)
    fn mode(&self, path: &Utf8Path) -> io::Result<u32>;

    /// Who owns a file, dir or symlink (not followed)
    fn uid(&self, path: &Utf8Path) -> io::Result<u32>;

    /// What a file, dir or symlink (not followed) takes on disk
    fn allocated(&self, path: &Utf8Path) -> io::Result<Allocated>;

    /// Removes a file or fails if it does not exist
    fn remove_file(&self, path: &Utf8Path) -> io::Result<()>;

    /// Returns false on permission errors and through broken symlinks
    #[must_use]
    fn exists(&self, path: &Utf8Path) -> bool;

    /// Returns false on permission errors or if it does not exist
    #[must_use]
    fn is_dir(&self, path: &Utf8Path) -> bool;

    /// Returns false on permission errors or if it does not exist
    #[must_use]
    fn is_symlink(&self, path: &Utf8Path) -> bool;

    /// Returns false on permission errors or if it does not exist (symlinks not followed)
    #[must_use]
    fn is_socket(&self, path: &Utf8Path) -> bool;

    /// File names of `path`'s entries, in unspecified order.
    fn read_dir(&self, path: &Utf8Path) -> io::Result<Vec<Utf8PathBuf>>;

    /// Digests the contents of a file
    fn sha256<'a>(&'a self, path: &'a Utf8Path) -> BoxFuture<'a, io::Result<String>>;
}
