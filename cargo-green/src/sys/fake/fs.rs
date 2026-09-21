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
    cwds: Vec<Utf8PathBuf>,               // Pushd stack
}

impl Inner {
    fn resolve(&self, path: &Utf8Path) -> Utf8PathBuf {
        let Some(cwd) = self.cwds.last() else { return path.to_owned() };
        cwd.join(path.strip_prefix("/").expect("/-started"))
    }

    fn insert_file(&mut self, path: &Utf8Path, contents: String) {
        self.insert_dirs(path.parent().unwrap_or(path));
        let _ = self.files.insert(path.to_owned(), contents);
    }

    fn insert_dirs(&mut self, path: &Utf8Path) {
        for dir in path.ancestors() {
            self.dirs.insert(dir.to_owned());
        }
    }
}

impl FakeFs {
    #[must_use]
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self { inner: Mutex::new(Inner::default()) })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }

    pub(crate) fn pushd(&self, dir: impl AsRef<Utf8Path>) {
        let mut inner = self.lock();
        let dir = inner.resolve(dir.as_ref());
        inner.insert_dirs(&dir);
        inner.cwds.push(dir);
    }

    pub(crate) fn popd(&self) {
        let popped = self.lock().cwds.pop();
        assert!(popped.is_some(), "popd: empty directory stack");
    }

    pub(crate) fn file(&self, path: impl AsRef<Utf8Path>, contents: impl Into<String>) {
        let mut inner = self.lock();
        let path = inner.resolve(path.as_ref());
        inner.insert_file(&path, contents.into());
    }

    pub(crate) fn mkdir(&self, path: impl AsRef<Utf8Path>) {
        let mut inner = self.lock();
        let path = inner.resolve(path.as_ref());
        inner.insert_dirs(&path);
    }

    #[must_use]
    pub(crate) fn read(&self, path: impl AsRef<Utf8Path>) -> Option<String> {
        self.lock().files.get(path.as_ref()).cloned()
    }

    #[expect(unused)]
    #[must_use]
    pub(crate) fn written(&self) -> Vec<Utf8PathBuf> {
        self.lock().files.keys().cloned().collect()
    }

    #[must_use]
    fn missing(path: &Utf8Path) -> io::Error {
        io::Error::new(io::ErrorKind::NotFound, format!("no such fake file: {path}"))
    }
}

#[test]
fn usage() {
    let fs = FakeFs::new();
    fs.pushd("/home/bla");
    fs.file("/some.thing", "data\n");
    fs.mkdir("/subdir");
    fs.popd();
    fs.file("/other/thing", "boop\n");
    fs.mkdir("/topdir");
    assert_eq!(fs.read_dir("/".into()).unwrap(), ["/home", "/other", "/topdir"]);
    assert_eq!(
        fs.read_dir("/home/bla".into()).unwrap(),
        ["/home/bla/some.thing", "/home/bla/subdir"]
    );
    assert_eq!(fs.read_dir("/other/".into()).unwrap(), ["/other/thing"]);
}

impl Fs for FakeFs {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        self.read(path).ok_or_else(|| Self::missing(path))
    }

    fn write(&self, path: &Utf8Path, data: &str) -> io::Result<()> {
        self.lock().insert_file(path, data.to_owned());
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
        self.lock().insert_dirs(path);
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
