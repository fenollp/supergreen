use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
};

use anyhow::{Result, anyhow};
use futures::future::LocalBoxFuture;

use crate::{green::Green, image_uri::ImageUri, runner::Runner, sys::Images};

/// A canned digest per resolution source, plus a log of which were consulted.
pub(crate) struct FakeImages {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    digests: HashMap<DigestSource, String>,
    consulted: Vec<DigestSource>, // A log
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum DigestSource {
    Builder,
    Local,
    Remote,
}

impl FakeImages {
    #[must_use]
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self { inner: Mutex::default() })
    }

    #[must_use]
    pub(crate) fn with_sources<Str: AsRef<str>>(
        digests: impl IntoIterator<Item = (DigestSource, Str)>,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                digests: digests.into_iter().map(|(k, v)| (k, v.as_ref().into())).collect(),
                ..Default::default()
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The sources that were queried, in order.
    #[must_use]
    pub(crate) fn consulted(&self) -> Vec<DigestSource> {
        self.lock().consulted.clone()
    }

    async fn maybe(&self, source: DigestSource, img: &ImageUri) -> Option<ImageUri> {
        let mut inner = self.lock();
        inner.consulted.push(source);
        inner.digests.get(&source).map(|digest| img.lock(digest))
    }
}

impl Images for FakeImages {
    fn lock_from_builder_cache<'a>(
        &'a self,
        _green: &'a Green,
        img: &'a ImageUri,
    ) -> LocalBoxFuture<'a, Result<Option<ImageUri>>> {
        Box::pin(async move { Ok(self.maybe(DigestSource::Builder, img).await) })
    }

    fn lock_from_image_cache<'a>(
        &'a self,
        _green: &'a Green,
        img: &'a ImageUri,
    ) -> LocalBoxFuture<'a, Result<Option<ImageUri>>> {
        Box::pin(async move { Ok(self.maybe(DigestSource::Local, img).await) })
    }

    fn fetch_digest<'a>(
        &'a self,
        _runner: &'a Runner,
        img: &'a ImageUri,
    ) -> LocalBoxFuture<'a, Result<ImageUri>> {
        Box::pin(async move {
            self.maybe(DigestSource::Remote, img)
                .await
                .ok_or_else(|| anyhow!("no fake remote digest for {img}"))
        })
    }
}
