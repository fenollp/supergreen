use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Cursor, Write},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, SystemTime},
};

use camino::{Utf8Path, Utf8PathBuf};
use futures::future::BoxFuture;

use crate::sys::{Allocated, AtomicWriter, Fs, Spool};

/// What a fake file takes on disk, rounded up to: a block
const BLOCK: u64 = 4096;

pub(crate) struct FakeFs {
    inner: Arc<Mutex<Inner>>,
}

struct FakeAtomicWriter {
    inner: Arc<Mutex<Inner>>,
    path: Utf8PathBuf,
    mode: u32,
    contents: Vec<u8>,
}

impl Write for FakeAtomicWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.contents.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl AtomicWriter for FakeAtomicWriter {
    fn commit(self: Box<Self>) -> io::Result<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.writable(&self.path)?;
        inner.insert_file(&self.path, &self.contents);
        let _ = inner.modes.insert(self.path.clone(), self.mode);
        drop(inner);
        Ok(())
    }
}

#[derive(Default)]
struct Inner {
    files: BTreeMap<Utf8PathBuf, Vec<u8>>, // Path --> content
    dirs: BTreeSet<Utf8PathBuf>,           // Exist from file paths + mkdir
    symlinks: BTreeMap<Utf8PathBuf, Utf8PathBuf>, // Link --> original
    modes: BTreeMap<Utf8PathBuf, u32>,     // Set explicitly, else defaults
    cwds: Vec<Utf8PathBuf>,                // Pushd stack
    readonly: BTreeSet<Utf8PathBuf>,       // Dirs
    mtimes: BTreeMap<Utf8PathBuf, u64>,    // Set explicitly, else the epoch
    uids: BTreeMap<Utf8PathBuf, u32>,      // Set explicitly, else `OWNER`
    sockets: BTreeSet<Utf8PathBuf>,        // Also in `files`, so they get listed
    inodes: BTreeMap<Utf8PathBuf, u64>,    // Hard links: names of a same file share an inode
}

/// Who owns what, unless told otherwise
const OWNER: u32 = 1000;

impl Inner {
    fn writable(&self, path: &Utf8Path) -> io::Result<()> {
        if self.readonly.iter().any(|dir| path.starts_with(dir)) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("read-only: {path}"),
            ));
        }
        Ok(())
    }

    fn resolve(&self, path: &Utf8Path) -> Utf8PathBuf {
        let Some(cwd) = self.cwds.last() else { return path.to_owned() };
        cwd.join(path.strip_prefix("/").expect("/-started"))
    }

    fn insert_file(&mut self, path: &Utf8Path, contents: &[u8]) {
        self.insert_dirs(path.parent().unwrap_or(path));
        let _ = self.files.insert(path.to_owned(), contents.to_owned());
        // Every name of a hard-linked file shows the new contents
        if let Some(inode) = self.inodes.get(path).copied() {
            let names: Vec<_> =
                self.inodes.iter().filter(|(_, i)| **i == inode).map(|(p, _)| p.clone()).collect();
            for name in names {
                let _ = self.files.insert(name, contents.to_owned());
            }
        }
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
        Arc::new(Self { inner: Arc::new(Mutex::new(Inner::default())) })
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
        inner.insert_file(&path, contents.into().as_bytes());
    }

    pub(crate) fn mkdir(&self, path: impl AsRef<Utf8Path>) {
        let mut inner = self.lock();
        let path = inner.resolve(path.as_ref());
        inner.insert_dirs(&path);
    }

    /// Nothing can be written under `dir` (which exists).
    pub(crate) fn readonly(&self, dir: impl AsRef<Utf8Path>) {
        let mut inner = self.lock();
        let dir = inner.resolve(dir.as_ref());
        inner.insert_dirs(&dir);
        inner.readonly.insert(dir);
    }

    /// Sets when `path` was last modified.
    pub(crate) fn mtime(&self, path: impl AsRef<Utf8Path>, secs: u64) {
        let mut inner = self.lock();
        let path = inner.resolve(path.as_ref());
        let _ = inner.mtimes.insert(path, secs);
    }

    /// Makes `path` owned by `uid`.
    pub(crate) fn chown(&self, path: impl AsRef<Utf8Path>, uid: u32) {
        let mut inner = self.lock();
        let path = inner.resolve(path.as_ref());
        let _ = inner.uids.insert(path, uid);
    }

    /// Makes a socket of `path`.
    pub(crate) fn socket(&self, path: impl AsRef<Utf8Path>) {
        let mut inner = self.lock();
        let path = inner.resolve(path.as_ref());
        inner.insert_file(&path, b"");
        let _ = inner.sockets.insert(path);
    }

    #[must_use]
    pub(crate) fn read(&self, path: impl AsRef<Utf8Path>) -> Option<String> {
        let bytes = self.bytes(path.as_ref()).ok()?;
        Some(String::from_utf8(bytes).expect("fake file holds UTF-8"))
    }

    #[must_use]
    pub(crate) fn written(&self) -> Vec<Utf8PathBuf> {
        self.lock().files.keys().cloned().collect()
    }

    fn bytes(&self, path: &Utf8Path) -> io::Result<Vec<u8>> {
        self.lock().files.get(path).cloned().ok_or_else(|| Self::missing(path))
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

#[test]
fn hard_links_share_an_inode_and_contents() {
    let fs = FakeFs::new();
    fs.file("/a/f", "x".repeat(5000));
    fs.hard_link("/a/f".into(), "/b/g".into()).unwrap();
    assert_eq!(fs.read("/b/g").unwrap().len(), 5000);

    let (f, g) = (fs.allocated("/a/f".into()).unwrap(), fs.allocated("/b/g".into()).unwrap());
    assert_eq!((f.bytes, f.hard_linked.is_some()), (8192, true));
    assert_eq!(f, g);

    fs.write("/b/g".into(), "short").unwrap();
    assert_eq!(fs.read("/a/f").unwrap(), "short");

    fs.remove_file("/b/g".into()).unwrap();
    assert_eq!(fs.allocated("/a/f".into()).unwrap().hard_linked, None);
    assert_eq!(
        fs.hard_link("/nope".into(), "/x".into()).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
    assert_eq!(
        fs.hard_link("/a/f".into(), "/a/f".into()).unwrap_err().kind(),
        io::ErrorKind::AlreadyExists
    );
}

#[test]
fn owners_and_sockets() {
    let fs = FakeFs::new();
    fs.file("/d/f", "");
    fs.socket("/d/s");
    assert_eq!(fs.uid("/d/f".into()).unwrap(), 1000);
    fs.chown("/d/f", 0);
    assert_eq!(fs.uid("/d/f".into()).unwrap(), 0);
    assert!(fs.uid("/nope".into()).is_err());

    assert!(fs.is_socket("/d/s".into()));
    assert!(!fs.is_socket("/d/f".into()));
    assert_eq!(fs.read_dir("/d".into()).unwrap(), ["/d/f", "/d/s"]);
    fs.remove_file("/d/s".into()).unwrap();
    assert!(!fs.is_socket("/d/s".into()));
}

impl Fs for FakeFs {
    fn open(&self, path: &Utf8Path) -> io::Result<Box<dyn io::Read + Send>> {
        Ok(Box::new(Cursor::new(self.bytes(path)?)))
    }

    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        String::from_utf8(self.bytes(path)?)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    fn write(&self, path: &Utf8Path, data: &str) -> io::Result<()> {
        let mut inner = self.lock();
        inner.writable(path)?;
        inner.insert_file(path, data.as_bytes());
        drop(inner);
        Ok(())
    }

    fn write_atomic(&self, path: &Utf8Path, data: &str) -> io::Result<()> {
        self.write(path, data)
    }

    fn write_atomic_bytes(&self, path: &Utf8Path, data: &[u8], mode: u32) -> io::Result<()> {
        let mut inner = self.lock();
        inner.writable(path)?;
        inner.insert_file(path, data);
        let _ = inner.modes.insert(path.to_owned(), mode);
        drop(inner);
        Ok(())
    }

    fn atomic_writer(&self, path: &Utf8Path, mode: u32) -> io::Result<Box<dyn AtomicWriter>> {
        self.lock().writable(path)?;
        Ok(Box::new(FakeAtomicWriter {
            inner: Arc::clone(&self.inner),
            path: path.to_owned(),
            mode,
            contents: vec![],
        }))
    }

    fn spool(&self, path: &Utf8Path) -> io::Result<Box<dyn Spool>> {
        self.lock().writable(path)?;
        Ok(Box::new(Cursor::new(vec![])))
    }

    fn append(&self, path: &Utf8Path, data: &str) -> io::Result<()> {
        let mut inner = self.lock();
        inner.writable(path)?;
        let Some(file) = inner.files.get_mut(path) else { return Err(Self::missing(path)) };
        file.extend_from_slice(data.as_bytes());
        drop(inner);
        Ok(())
    }

    fn copy(&self, from: &Utf8Path, to: &Utf8Path) -> io::Result<()> {
        let contents = self.read_to_string(from)?;
        self.write(to, &contents)
    }

    fn create_dir_all(&self, path: &Utf8Path) -> io::Result<()> {
        let mut inner = self.lock();
        if !inner.dirs.contains(path) {
            inner.writable(path)?;
        }
        inner.insert_dirs(path);
        drop(inner);
        Ok(())
    }

    fn create_dir_all_mode(&self, path: &Utf8Path, mode: u32) -> io::Result<()> {
        self.create_dir_all(path)?;
        let _ = self.lock().modes.insert(path.to_owned(), mode);
        Ok(())
    }

    fn symlink_file(&self, original: &Utf8Path, link: &Utf8Path) -> io::Result<()> {
        let mut inner = self.lock();
        inner.writable(link)?;
        inner.insert_dirs(link.parent().unwrap_or(link));
        let _ = inner.symlinks.insert(link.to_owned(), original.to_owned());
        drop(inner);
        Ok(())
    }

    fn hard_link(&self, original: &Utf8Path, link: &Utf8Path) -> io::Result<()> {
        let mut inner = self.lock();
        let Some(contents) = inner.files.get(original).cloned() else {
            return Err(Self::missing(original));
        };
        if inner.files.contains_key(link) || inner.dirs.contains(link) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("exists: {link}")));
        }
        inner.writable(link)?;
        let next = inner.inodes.values().max().map_or(1, |max| max + 1);
        let inode = *inner.inodes.entry(original.to_owned()).or_insert(next);
        let _ = inner.inodes.insert(link.to_owned(), inode);
        inner.insert_file(link, &contents);
        drop(inner);
        Ok(())
    }

    fn symlink_dir(&self, original: &Utf8Path, link: &Utf8Path) -> io::Result<()> {
        if self.exists(link) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("exists: {link}")));
        }
        let mut inner = self.lock();
        inner.writable(link)?;
        let _ = inner.symlinks.insert(link.to_owned(), original.to_owned());
        drop(inner);
        Ok(())
    }

    fn remove_symlink_dir(&self, link: &Utf8Path) -> io::Result<()> {
        let mut inner = self.lock();
        inner.writable(link)?;
        let Some(_) = inner.symlinks.remove(link) else { return Err(Self::missing(link)) };
        drop(inner);
        Ok(())
    }

    fn read_link(&self, link: &Utf8Path) -> io::Result<Utf8PathBuf> {
        self.lock().symlinks.get(link).cloned().ok_or_else(|| Self::missing(link))
    }

    fn canonicalize(&self, path: &Utf8Path) -> io::Result<Utf8PathBuf> {
        let inner = self.lock();
        let mut path = path.to_owned();
        while let Some(target) = inner.symlinks.get(&path) {
            path = target.clone();
        }
        if inner.files.contains_key(&path) || inner.dirs.contains(&path) {
            return Ok(path);
        }
        drop(inner);
        Err(Self::missing(&path))
    }

    fn modified(&self, path: &Utf8Path) -> io::Result<SystemTime> {
        if !self.exists(path) {
            return Err(Self::missing(path));
        }
        let secs = self.lock().mtimes.get(path).copied().unwrap_or_default();
        Ok(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
    }

    fn mode(&self, path: &Utf8Path) -> io::Result<u32> {
        let inner = self.lock();
        if let Some(mode) = inner.modes.get(path) {
            return Ok(*mode);
        }
        if inner.symlinks.contains_key(path) {
            return Ok(0o777);
        }
        if inner.dirs.contains(path) {
            return Ok(0o755);
        }
        if inner.files.contains_key(path) {
            return Ok(0o644);
        }
        drop(inner);
        Err(Self::missing(path))
    }

    fn uid(&self, path: &Utf8Path) -> io::Result<u32> {
        let inner = self.lock();
        if let Some(uid) = inner.uids.get(path) {
            return Ok(*uid);
        }
        if inner.files.contains_key(path)
            || inner.dirs.contains(path)
            || inner.symlinks.contains_key(path)
        {
            return Ok(OWNER);
        }
        drop(inner);
        Err(Self::missing(path))
    }

    fn allocated(&self, path: &Utf8Path) -> io::Result<Allocated> {
        let inner = self.lock();
        if inner.symlinks.contains_key(path) {
            return Ok(Allocated { bytes: 0, is_dir: false, hard_linked: None });
        }
        if inner.dirs.contains(path) {
            return Ok(Allocated { bytes: BLOCK, is_dir: true, hard_linked: None });
        }
        if let Some(contents) = inner.files.get(path) {
            let inode = inner.inodes.get(path).copied();
            let names = inode.map_or(0, |i| inner.inodes.values().filter(|n| **n == i).count());
            return Ok(Allocated {
                bytes: (contents.len() as u64).div_ceil(BLOCK) * BLOCK,
                is_dir: false,
                hard_linked: inode.filter(|_| names > 1).map(|inode| (0, inode)),
            });
        }
        drop(inner);
        Err(Self::missing(path))
    }

    fn remove_file(&self, path: &Utf8Path) -> io::Result<()> {
        let mut inner = self.lock();
        let Some(_) = inner.files.remove(path) else { return Err(Self::missing(path)) };
        let _ = inner.inodes.remove(path);
        let _ = inner.sockets.remove(path);
        drop(inner);
        Ok(())
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        let inner = self.lock();
        inner.files.contains_key(path)
            || inner.dirs.contains(path)
            || inner.symlinks.contains_key(path)
    }

    fn is_dir(&self, path: &Utf8Path) -> bool {
        self.lock().dirs.contains(path)
    }

    fn is_symlink(&self, path: &Utf8Path) -> bool {
        self.lock().symlinks.contains_key(path)
    }

    fn is_socket(&self, path: &Utf8Path) -> bool {
        self.lock().sockets.contains(path)
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
        names.extend(children(&mut inner.symlinks.keys()));
        names.extend(children(&mut inner.dirs.iter()));
        drop(inner);
        names.sort();
        names.dedup();
        Ok(names)
    }

    fn sha256<'a>(&'a self, path: &'a Utf8Path) -> BoxFuture<'a, io::Result<String>> {
        Box::pin(async move { self.read_to_string(path).map(sha256::digest) })
    }
}
