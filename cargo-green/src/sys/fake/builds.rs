use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::Result;
use camino::{Utf8Path, Utf8PathBuf};
use futures::future::LocalBoxFuture;
use indexmap::IndexSet;

use crate::{
    build::{Built, Effects},
    green::Green,
    md::BuildContext,
    stage::Stage,
    sys::Builds,
};

/// A runner whose builds report `stdout` and `stderr`, as their call left them in its
/// result, and having written `written`.
pub(crate) struct FakeBuilds {
    stdout: String,
    stderr: String,
    written: Vec<Utf8PathBuf>,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    containerfiles: Vec<Utf8PathBuf>, // Built, in order.
}

impl FakeBuilds {
    #[must_use]
    pub(crate) fn new() -> Arc<Self> {
        Self::with_stdio("", "", [""; 0])
    }

    #[must_use]
    pub(crate) fn with_written<P: AsRef<Utf8Path>>(
        files: impl IntoIterator<Item = P>,
    ) -> Arc<Self> {
        Self::with_stdio("", "", files)
    }

    #[must_use]
    pub(crate) fn with_stdio<P: AsRef<Utf8Path>>(
        stdout: &str,
        stderr: &str,
        written: impl IntoIterator<Item = P>,
    ) -> Arc<Self> {
        Arc::new(Self {
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
            written: written.into_iter().map(|x| x.as_ref().into()).collect(),
            inner: Mutex::default(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }

    #[must_use]
    pub(crate) fn built(&self) -> Vec<Utf8PathBuf> {
        self.lock().containerfiles.clone()
    }
}

impl Builds for FakeBuilds {
    fn build_out<'a>(
        &'a self,
        green: &'a Green,
        containerfile: &'a Utf8Path,
        target: &'a Stage,
        _contexts: &'a IndexSet<BuildContext>,
        out_dir: &'a Utf8Path,
    ) -> LocalBoxFuture<'a, Built> {
        Box::pin(async move {
            // As the result's extracted
            for written in &self.written {
                crate::sys::fs().write(&out_dir.join(written), "").unwrap();
            }
            self.lock().containerfiles.push(containerfile.to_owned());
            let call = format!("docker buildx build --target={target}");
            let envs = format!("{}=\"1\"", DOCKER_BUILDKIT!());
            let effects = Effects::from_stdio(
                &green.paths,
                &self.stdout,
                &self.stderr,
                self.written.clone(),
                0,
            );
            Built { call, envs, effects, result: None, built: Ok(()) }
        })
    }

    fn build_cacheonly<'a>(
        &'a self,
        _green: &'a Green,
        containerfile: &'a Utf8Path,
        _target: &'a Stage,
    ) -> LocalBoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.lock().containerfiles.push(containerfile.to_owned());
            Ok(())
        })
    }
}
