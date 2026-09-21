use std::{fs, io, io::Write};

use atomic_write_file::AtomicWriteFile;
use camino::{Utf8Path, Utf8PathBuf};
use futures::future::LocalBoxFuture;

use crate::sys::Fs;

pub(crate) struct RealFs;

impl Fs for RealFs {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        fs::read_to_string(path)
    }

    fn write(&self, path: &Utf8Path, data: &str) -> io::Result<()> {
        fs::write(path, data)
    }

    fn write_atomic(&self, path: &Utf8Path, data: &str) -> io::Result<()> {
        let mut file = AtomicWriteFile::open(path)?;
        file.write_all(data.as_bytes())?;
        file.commit()
    }

    fn append(&self, path: &Utf8Path, data: &str) -> io::Result<()> {
        let mut file = fs::OpenOptions::new().append(true).open(path)?;
        file.write_all(data.as_bytes())
    }

    fn copy(&self, from: &Utf8Path, to: &Utf8Path) -> io::Result<()> {
        let _ = fs::copy(from, to)?;
        Ok(())
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

    fn read_dir(&self, path: &Utf8Path) -> io::Result<Vec<Utf8PathBuf>> {
        path.read_dir_utf8()?.map(|entry| Ok(entry?.into_path())).collect()
    }

    fn sha256<'a>(&'a self, path: &'a Utf8Path) -> LocalBoxFuture<'a, io::Result<String>> {
        Box::pin(sha256::try_async_digest(path))
    }
}
