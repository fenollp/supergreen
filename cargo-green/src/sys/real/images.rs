use anyhow::Result;
use futures::future::LocalBoxFuture;

use crate::{green::Green, image_uri::ImageUri, runner::Runner, sys::Images};

pub(crate) struct RealImages;

impl Images for RealImages {
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

    fn fetch_digest<'a>(
        &'a self,
        _runner: &'a Runner,
        img: &'a ImageUri,
    ) -> LocalBoxFuture<'a, Result<ImageUri>> {
        Box::pin(crate::image_uri::real_fetch_digest(img))
    }
}
