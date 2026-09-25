use anyhow::Result;
use futures::future::LocalBoxFuture;

use crate::{green::Green, image_uri::ImageUri, runner::Runner};

/// OCI images resolution operations.
///
/// First, try builder's build cache then runner's local one and finally read remote.
pub(crate) trait Images: Send + Sync {
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

    /// Query the remote registry. Hits the network.
    fn fetch_digest<'a>(
        &'a self,
        runner: &'a Runner,
        img: &'a ImageUri,
    ) -> LocalBoxFuture<'a, Result<ImageUri>>;
}
