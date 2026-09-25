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

pub(crate) struct FakeBuilds {
    effects: Effects, // Only set at creation
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    containerfiles: Vec<Utf8PathBuf>, // Built, in order.
}

impl FakeBuilds {
    #[must_use]
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self { effects: Effects::default(), inner: Mutex::default() })
    }

    #[must_use]
    pub(crate) fn wrote<P: AsRef<Utf8Path>>(files: impl IntoIterator<Item = P>) -> Arc<Self> {
        Self::set_and_wrote::<&str, &str, P>([], files)
    }

    #[must_use]
    pub(crate) fn set_and_wrote<K: AsRef<str>, V: AsRef<str>, P: AsRef<Utf8Path>>(
        envs: impl IntoIterator<Item = (K, V)>,
        files: impl IntoIterator<Item = P>,
    ) -> Arc<Self> {
        Arc::new(Self {
            effects: Effects {
                written: files.into_iter().map(|x| x.as_ref().into()).collect(),
                rustc_envs: envs
                    .into_iter()
                    .map(|(k, v)| (k.as_ref().into(), v.as_ref().into()))
                    .collect(),
                ..Default::default()
            },
            inner: Mutex::default(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[must_use]
    pub(crate) fn built(&self) -> Vec<Utf8PathBuf> {
        self.lock().containerfiles.clone()
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
    ) -> LocalBoxFuture<'a, Built> {
        Box::pin(async move {
            self.lock().containerfiles.push(containerfile.to_owned());
            let call = format!("docker buildx build --target={target}");
            let envs = format!("{}=\"1\"", DOCKER_BUILDKIT!());
            let effects = self.effects.clone();
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
