use anyhow::Result;
use futures::future::LocalBoxFuture;

use crate::{green::Green, image_uri::ImageUri, runner::Runner};

/// Resolution of an un-pinned image URI to a digest-locked one.
///
/// Three sources, tried in that order by [`Green::maybe_lock_image`] then
/// [`crate::image_uri::fetch_digest`]: the builder's build cache, the runner's
/// local image cache, and finally the remote registry API.
pub(crate) trait Images: Send + Sync {
    /// Query the remote registry. Hits the network.
    fn fetch_digest<'a>(
        &'a self,
        runner: &'a Runner,
        img: &'a ImageUri,
    ) -> LocalBoxFuture<'a, Result<ImageUri>>;

    /// Query the builder's build cache.
    fn lock_from_builder_cache<'a>(
        &'a self,
        green: &'a Green,
        img: &'a ImageUri,
    ) -> LocalBoxFuture<'a, Result<Option<ImageUri>>>;

    /// Query the runner's local image cache.
    fn lock_from_image_cache<'a>(
        &'a self,
        green: &'a Green,
        img: &'a ImageUri,
    ) -> LocalBoxFuture<'a, Result<Option<ImageUri>>>;
}

pub(crate) struct RealImages;

impl Images for RealImages {
    fn fetch_digest<'a>(
        &'a self,
        _runner: &'a Runner,
        img: &'a ImageUri,
    ) -> LocalBoxFuture<'a, Result<ImageUri>> {
        Box::pin(crate::image_uri::real_fetch_digest(img))
    }

    fn lock_from_builder_cache<'a>(
        &'a self,
        green: &'a Green,
        img: &'a ImageUri,
    ) -> LocalBoxFuture<'a, Result<Option<ImageUri>>> {
        Box::pin(green.real_lock_from_builder_cache(img))
    }

    fn lock_from_image_cache<'a>(
        &'a self,
        green: &'a Green,
        img: &'a ImageUri,
    ) -> LocalBoxFuture<'a, Result<Option<ImageUri>>> {
        Box::pin(green.real_lock_from_image_cache(img))
    }
}
