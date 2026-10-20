use std::{
    fs, io,
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
};

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

    fn write_atomic_bytes(&self, path: &Utf8Path, data: &[u8], mode: u32) -> io::Result<()> {
        let mut opts = AtomicWriteFile::options();
        opts.mode(mode);
        let mut file = opts.open(path)?;
        file.write_all(data)?;
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

    fn create_dir_all_mode(&self, path: &Utf8Path, mode: u32) -> io::Result<()> {
        fs::DirBuilder::new().mode(mode).recursive(true).create(path)
    }

    fn symlink_file(&self, original: &Utf8Path, link: &Utf8Path) -> io::Result<()> {
        let _ = symlink::remove_symlink_file(link);
        symlink::symlink_file(original, link)
    }

    fn mode(&self, path: &Utf8Path) -> io::Result<u32> {
        Ok(path.symlink_metadata()?.mode() & 0o777)
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

    fn is_symlink(&self, path: &Utf8Path) -> bool {
        path.is_symlink()
    }

    fn read_dir(&self, path: &Utf8Path) -> io::Result<Vec<Utf8PathBuf>> {
        path.read_dir_utf8()?.map(|entry| Ok(entry?.into_path())).collect()
    }

    fn sha256<'a>(&'a self, path: &'a Utf8Path) -> LocalBoxFuture<'a, io::Result<String>> {
        Box::pin(sha256::try_async_digest(path))
    }
}
