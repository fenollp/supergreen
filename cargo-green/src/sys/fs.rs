use std::{fs, io, path::Path};

use atomic_write_file::AtomicWriteFile;
use camino::Utf8Path;
use futures::future::LocalBoxFuture;

/// Filesystem operations on the Containerfile generation path.
///
/// Deliberately narrow: startup-only IO (`Paths::setup`, `setup_dirs`,
/// `maybe_arrange_cratesio_index`) and the tar/streaming plumbing in
/// [`crate::build`] and [`crate::cache`] stay concrete, as no test reaches them.
///
/// Primitives return [`io::Result`] so callers keep formatting their own errors,
/// and so [`io::ErrorKind`] stays inspectable (see [`crate::md::Md::from_file`]).
pub(crate) trait Fs: Send + Sync {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String>;

    fn write(&self, path: &Utf8Path, data: &str) -> io::Result<()>;

    /// Write through a temporary file then `rename`, so readers never see a partial file.
    fn write_atomic(&self, path: &Utf8Path, data: &str) -> io::Result<()>;

    fn append(&self, path: &Utf8Path, data: &str) -> io::Result<()>;

    fn copy(&self, from: &Utf8Path, to: &Utf8Path) -> io::Result<()>;

    fn create_dir_all(&self, path: &Utf8Path) -> io::Result<()>;

    fn remove_file(&self, path: &Utf8Path) -> io::Result<()>;

    fn exists(&self, path: &Utf8Path) -> bool;

    fn is_dir(&self, path: &Utf8Path) -> bool;

    /// File names of `path`'s entries, in unspecified order.
    fn read_dir(&self, path: &Utf8Path) -> io::Result<Vec<String>>;

    fn sha256<'a>(&'a self, path: &'a Utf8Path) -> LocalBoxFuture<'a, io::Result<String>>;
}

pub(crate) struct RealFs;

impl Fs for RealFs {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        fs::read_to_string(path)
    }

    fn write(&self, path: &Utf8Path, data: &str) -> io::Result<()> {
        fs::write(path, data)
    }

    fn write_atomic(&self, path: &Utf8Path, data: &str) -> io::Result<()> {
        use std::io::Write;
        let mut file = AtomicWriteFile::open(path)?;
        file.write_all(data.as_bytes())?;
        file.commit()
    }

    fn append(&self, path: &Utf8Path, data: &str) -> io::Result<()> {
        use std::io::Write;
        let mut file = fs::OpenOptions::new().append(true).open(path)?;
        file.write_all(data.as_bytes())
    }

    fn copy(&self, from: &Utf8Path, to: &Utf8Path) -> io::Result<()> {
        fs::copy(from, to).map(drop)
    }

    fn create_dir_all(&self, path: &Utf8Path) -> io::Result<()> {
        fs::create_dir_all(path)
    }

    fn remove_file(&self, path: &Utf8Path) -> io::Result<()> {
        fs::remove_file(path)
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        path.exists()
    }

    fn is_dir(&self, path: &Utf8Path) -> bool {
        path.is_dir()
    }

    fn read_dir(&self, path: &Utf8Path) -> io::Result<Vec<String>> {
        fs::read_dir(path)?
            .map(|entry| {
                let entry = entry?;
                Path::new(&entry.file_name())
                    .to_str()
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| io::Error::other(format!("{path} has a non-utf-8 entry")))
            })
            .collect()
    }

    fn sha256<'a>(&'a self, path: &'a Utf8Path) -> LocalBoxFuture<'a, io::Result<String>> {
        Box::pin(sha256::try_async_digest(path))
    }
}
