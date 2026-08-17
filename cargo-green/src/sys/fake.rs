//! In-memory stand-ins for every side effect, for use with [`super::install`].

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    sync::{Arc, Mutex, PoisonError},
};

use anyhow::{Result, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use futures::future::LocalBoxFuture;
use indexmap::IndexSet;

use crate::{
    build::Effects,
    cache::result::ResultWriter,
    green::Green,
    image_uri::ImageUri,
    md::BuildContext,
    runner::Runner,
    stage::Stage,
    sys::{Builds, Fs, Git, Images, Sys},
};

impl Sys {
    /// A bundle where nothing touches the outside world.
    ///
    /// Every capability starts empty: a [`FakeFs`] with no files, a [`FakeGit`] that
    /// finds no repository, [`FakeImages`] that resolves no digest, and [`FakeBuilds`]
    /// that reports a successful build with no effects.
    ///
    /// Keep a handle on whichever ones the test seeds or asserts against:
    ///
    /// ```ignore
    /// let fs = Arc::new(FakeFs::default());
    /// fs.file("/work/Cargo.toml", "[package]\nname = \"x\"\n");
    /// let _guard = install(Sys { fs: Arc::clone(&fs) as _, ..Sys::fake() });
    /// ```
    #[must_use]
    pub(crate) fn fake() -> Self {
        Self {
            fs: Arc::new(FakeFs::default()),
            git: Arc::new(FakeGit::default()),
            images: Arc::new(FakeImages::default()),
            builds: Arc::new(FakeBuilds::default()),
        }
    }
}

// ---------------------------------------------------------------- filesystem

/// A flat map of paths to contents. Directories exist implicitly, from the paths
/// of the files under them, plus any added explicitly with [`FakeFs::mkdir`].
#[derive(Default)]
pub(crate) struct FakeFs {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    files: BTreeMap<Utf8PathBuf, String>,
    dirs: BTreeSet<Utf8PathBuf>,
}

impl FakeFs {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Seed a file, creating its parent directories.
    pub(crate) fn file(&self, path: impl AsRef<Utf8Path>, contents: impl Into<String>) -> &Self {
        let path = path.as_ref();
        let mut inner = self.lock();
        for parent in path.ancestors().skip(1) {
            inner.dirs.insert(parent.to_owned());
        }
        let _ = inner.files.insert(path.to_owned(), contents.into());
        drop(inner);
        self
    }

    /// Seed an empty directory.
    pub(crate) fn mkdir(&self, path: impl AsRef<Utf8Path>) -> &Self {
        let path = path.as_ref();
        let mut inner = self.lock();
        for dir in path.ancestors() {
            inner.dirs.insert(dir.to_owned());
        }
        drop(inner);
        self
    }

    /// Contents of `path`, if it was written or seeded.
    #[must_use]
    pub(crate) fn read(&self, path: impl AsRef<Utf8Path>) -> Option<String> {
        self.lock().files.get(path.as_ref()).cloned()
    }

    /// Every file path, sorted. Handy for asserting what a run touched.
    #[must_use]
    pub(crate) fn written(&self) -> Vec<Utf8PathBuf> {
        self.lock().files.keys().cloned().collect()
    }

    fn missing(path: &Utf8Path) -> io::Error {
        io::Error::new(io::ErrorKind::NotFound, format!("no such fake file: {path}"))
    }
}

impl Fs for FakeFs {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        self.lock().files.get(path).cloned().ok_or_else(|| Self::missing(path))
    }

    fn write(&self, path: &Utf8Path, data: &str) -> io::Result<()> {
        let _ = self.file(path, data);
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
        let _ = self.mkdir(path);
        Ok(())
    }

    fn remove_file(&self, path: &Utf8Path) -> io::Result<()> {
        self.lock().files.remove(path).map(drop).ok_or_else(|| Self::missing(path))
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        let inner = self.lock();
        inner.files.contains_key(path) || inner.dirs.contains(path)
    }

    fn is_dir(&self, path: &Utf8Path) -> bool {
        self.lock().dirs.contains(path)
    }

    fn read_dir(&self, path: &Utf8Path) -> io::Result<Vec<String>> {
        let inner = self.lock();
        if !inner.dirs.contains(path) {
            return Err(Self::missing(path));
        }
        let children = |it: &mut dyn Iterator<Item = &Utf8PathBuf>| -> Vec<String> {
            it.filter_map(|p| p.strip_prefix(path).ok())
                .filter_map(|rest| rest.components().next())
                .map(|first| first.to_string())
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
        // Real digest of the fake contents: deterministic, and still a valid sha256.
        Box::pin(async move { self.read_to_string(path).map(sha256::digest) })
    }
}

// ----------------------------------------------------------------------- git

/// Maps a checkout directory to the `FETCH_HEAD` its git db would have.
#[derive(Default)]
pub(crate) struct FakeGit {
    pub(crate) heads: BTreeMap<Utf8PathBuf, Utf8PathBuf>,
}

impl Git for FakeGit {
    fn fetch_head(&self, pkg_manifest_dir: &Utf8Path) -> Result<Utf8PathBuf> {
        self.heads
            .get(pkg_manifest_dir)
            .cloned()
            .ok_or_else(|| anyhow!("no fake git repository for {pkg_manifest_dir}"))
    }
}

// -------------------------------------------------------------------- images

/// A canned digest per resolution source, plus a log of which were consulted.
#[derive(Default)]
pub(crate) struct FakeImages {
    pub(crate) builder_cache: Option<String>,
    pub(crate) image_cache: Option<String>,
    pub(crate) remote: Option<String>,
    pub(crate) consulted: Mutex<Vec<&'static str>>,
}

impl FakeImages {
    /// The sources that were queried, in order.
    #[must_use]
    pub(crate) fn consulted(&self) -> Vec<&'static str> {
        self.consulted.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn note(&self, source: &'static str) {
        self.consulted.lock().unwrap_or_else(PoisonError::into_inner).push(source);
    }
}

impl Images for FakeImages {
    fn fetch_digest<'a>(
        &'a self,
        _runner: &'a Runner,
        img: &'a ImageUri,
    ) -> LocalBoxFuture<'a, Result<ImageUri>> {
        Box::pin(async move {
            self.note("remote");
            match self.remote {
                Some(ref digest) => Ok(img.lock(digest)),
                None => Err(anyhow!("no fake remote digest for {img}")),
            }
        })
    }

    fn lock_from_builder_cache<'a>(
        &'a self,
        _green: &'a Green,
        img: &'a ImageUri,
    ) -> LocalBoxFuture<'a, Result<Option<ImageUri>>> {
        Box::pin(async move {
            self.note("builder-cache");
            Ok(self.builder_cache.as_ref().map(|digest| img.lock(digest)))
        })
    }

    fn lock_from_image_cache<'a>(
        &'a self,
        _green: &'a Green,
        img: &'a ImageUri,
    ) -> LocalBoxFuture<'a, Result<Option<ImageUri>>> {
        Box::pin(async move {
            self.note("image-cache");
            Ok(self.image_cache.as_ref().map(|digest| img.lock(digest)))
        })
    }
}

// -------------------------------------------------------------------- builds

/// Reports a build without running one, yielding whatever [`Effects`] it was given.
#[derive(Default)]
pub(crate) struct FakeBuilds {
    pub(crate) effects: Effects,
    /// Containerfile paths passed to `build_out`, in order.
    pub(crate) built: Mutex<Vec<Utf8PathBuf>>,
}

impl FakeBuilds {
    #[must_use]
    pub(crate) fn built(&self) -> Vec<Utf8PathBuf> {
        self.built.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl Builds for FakeBuilds {
    fn build_out<'a>(
        &'a self,
        _green: &'a Green,
        containerfile: &'a Utf8Path,
        target: &'a Stage,
        _contexts: &'a IndexSet<BuildContext>,
        _out_dir: &'a Utf8Path,
    ) -> LocalBoxFuture<'a, (String, String, Effects, Option<ResultWriter>, Result<()>)> {
        Box::pin(async move {
            self.built
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(containerfile.to_owned());
            let call = format!("docker buildx build --target={target}");
            let envs = format!("{}=\"1\"", DOCKER_BUILDKIT!());
            (call, envs, self.effects.clone(), None, Ok(()))
        })
    }

    fn build_cacheonly<'a>(
        &'a self,
        _green: &'a Green,
        containerfile: &'a Utf8Path,
        _target: &'a Stage,
    ) -> LocalBoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.built
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(containerfile.to_owned());
            Ok(())
        })
    }
}
