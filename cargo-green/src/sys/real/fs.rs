use std::{
    fs, io,
    io::Write,
    os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt},
    time::SystemTime,
};

use atomic_write_file::AtomicWriteFile;
use camino::{Utf8Path, Utf8PathBuf};
use futures::future::BoxFuture;

use crate::sys::{Allocated, AtomicWriter, Fs, Spool, fs::TAR_BLOCK};

pub(crate) struct RealFs;

impl AtomicWriter for AtomicWriteFile {
    fn commit(self: Box<Self>) -> io::Result<()> {
        Self::commit(*self)
    }
}

impl Fs for RealFs {
    fn open(&self, path: &Utf8Path) -> io::Result<Box<dyn io::Read + Send>> {
        Ok(Box::new(io::BufReader::new(fs::File::open(path)?)))
    }

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

    fn atomic_writer(&self, path: &Utf8Path, mode: u32) -> io::Result<Box<dyn AtomicWriter>> {
        let mut opts = AtomicWriteFile::options();
        opts.mode(mode);
        Ok(Box::new(opts.open(path)?))
    }

    fn spool(&self, path: &Utf8Path) -> io::Result<Box<dyn Spool>> {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        fs::remove_file(path)?;
        Ok(Box::new(file))
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

    fn hard_link(&self, original: &Utf8Path, link: &Utf8Path) -> io::Result<()> {
        fs::hard_link(original, link)
    }

    fn symlink_dir(&self, original: &Utf8Path, link: &Utf8Path) -> io::Result<()> {
        symlink::symlink_dir(original, link)
    }

    fn remove_symlink_dir(&self, link: &Utf8Path) -> io::Result<()> {
        symlink::remove_symlink_dir(link)
    }

    fn read_link(&self, link: &Utf8Path) -> io::Result<Utf8PathBuf> {
        link.read_link_utf8()
    }

    fn canonicalize(&self, path: &Utf8Path) -> io::Result<Utf8PathBuf> {
        path.canonicalize_utf8()
    }

    fn modified(&self, path: &Utf8Path) -> io::Result<SystemTime> {
        path.metadata()?.modified()
    }

    fn mode(&self, path: &Utf8Path) -> io::Result<u32> {
        Ok(path.symlink_metadata()?.mode() & 0o777)
    }

    fn uid(&self, path: &Utf8Path) -> io::Result<u32> {
        Ok(path.symlink_metadata()?.uid())
    }

    fn allocated(&self, path: &Utf8Path) -> io::Result<Allocated> {
        let md = path.symlink_metadata()?;
        Ok(Allocated {
            bytes: md.blocks() * TAR_BLOCK, // st_blocks is in 512B units, whatever the filesystem's
            is_dir: md.is_dir(),
            hard_linked: (md.nlink() > 1 && !md.is_dir()).then(|| (md.dev(), md.ino())),
        })
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

    fn is_socket(&self, path: &Utf8Path) -> bool {
        path.symlink_metadata().is_ok_and(|md| md.file_type().is_socket())
    }

    fn read_dir(&self, path: &Utf8Path) -> io::Result<Vec<Utf8PathBuf>> {
        path.read_dir_utf8()?.map(|entry| Ok(entry?.into_path())).collect()
    }

    fn sha256<'a>(&'a self, path: &'a Utf8Path) -> BoxFuture<'a, io::Result<String>> {
        Box::pin(sha256::try_async_digest(path))
    }
}
