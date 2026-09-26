use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    sync::{Arc, Mutex, MutexGuard},
};

use camino::{Utf8Path, Utf8PathBuf};
use futures::future::LocalBoxFuture;

use crate::sys::Fs;

pub(crate) struct FakeFs {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    files: BTreeMap<Utf8PathBuf, String>, // Path --> content
    dirs: BTreeSet<Utf8PathBuf>,          // Exist from file paths + mkdir
}

impl FakeFs {
    #[must_use]
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self { inner: Mutex::new(Inner::default()) })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Seed a file, creating its parent directories.
    pub(crate) fn file(&self, path: impl AsRef<Utf8Path>, contents: impl Into<String>) {
        let path = path.as_ref();
        let mut inner = self.lock();
        for parent in path.ancestors().skip(1) {
            inner.dirs.insert(parent.to_owned());
        }
        let _ = inner.files.insert(path.to_owned(), contents.into());
    }

    pub(crate) fn mkdir(&self, path: impl AsRef<Utf8Path>) {
        let path = path.as_ref();
        let mut inner = self.lock();
        for dir in path.ancestors() {
            inner.dirs.insert(dir.to_owned());
        }
    }

    #[must_use]
    pub(crate) fn read(&self, path: impl AsRef<Utf8Path>) -> Option<String> {
        self.lock().files.get(path.as_ref()).cloned()
    }

    #[must_use]
    pub(crate) fn written(&self) -> Vec<Utf8PathBuf> {
        self.lock().files.keys().cloned().collect()
    }

    #[must_use]
    fn missing(path: &Utf8Path) -> io::Error {
        io::Error::new(io::ErrorKind::NotFound, format!("no such fake file: {path}"))
    }
}

impl Fs for FakeFs {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        self.read(path).ok_or_else(|| Self::missing(path))
    }

    fn write(&self, path: &Utf8Path, data: &str) -> io::Result<()> {
        self.file(path, data);
        Ok(())
    }

    fn write_atomic(&self, path: &Utf8Path, data: &str) -> io::Result<()> {
        self.write(path, data)
    }

    #[expect(clippy::significant_drop_tightening)]
    fn append(&self, path: &Utf8Path, data: &str) -> io::Result<()> {
        let mut inner = self.lock();
        let Some(file) = inner.files.get_mut(path) else { return Err(Self::missing(path)) };
        file.push_str(data);
        Ok(())
    }

    fn copy(&self, from: &Utf8Path, to: &Utf8Path) -> io::Result<()> {
        let contents = self.read_to_string(from)?;
        self.write(to, &contents)
    }

    fn create_dir_all(&self, path: &Utf8Path) -> io::Result<()> {
        self.mkdir(path);
        Ok(())
    }

    fn remove_file(&self, path: &Utf8Path) -> io::Result<()> {
        let Some(_) = self.lock().files.remove(path) else { return Err(Self::missing(path)) };
        Ok(())
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        let inner = self.lock();
        inner.files.contains_key(path) || inner.dirs.contains(path)
    }

    fn is_dir(&self, path: &Utf8Path) -> bool {
        self.lock().dirs.contains(path)
    }

    fn read_dir(&self, path: &Utf8Path) -> io::Result<Vec<Utf8PathBuf>> {
        let inner = self.lock();
        let true = inner.dirs.contains(path) else { return Err(Self::missing(path)) };
        let children = |it: &mut dyn Iterator<Item = &Utf8PathBuf>| -> Vec<Utf8PathBuf> {
            it.filter_map(|p| p.strip_prefix(path).ok())
                .filter_map(|rest| rest.components().next())
                .map(|first| path.join(first.as_str()))
                .collect()
        };
        let mut names = children(&mut inner.files.keys());
        names.extend(children(&mut inner.dirs.iter()));
        drop(inner);
        names.sort();
        names.dedup();
        Ok(names)
    }

    fn sha256<'a>(&'a self, path: &'a Utf8Path) -> LocalBoxFuture<'a, io::Result<String>> {
        Box::pin(async move { self.read_to_string(path).map(sha256::digest) })
    }
}
