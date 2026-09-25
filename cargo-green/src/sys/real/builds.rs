use anyhow::Result;
use camino::Utf8Path;
use futures::future::LocalBoxFuture;
use indexmap::IndexSet;

use crate::{build::Built, green::Green, md::BuildContext, stage::Stage, sys::Builds};

pub(crate) struct RealBuilds;

impl Builds for RealBuilds {
    fn build_out<'a>(
        &'a self,
        green: &'a Green,
        containerfile: &'a Utf8Path,
        target: &'a Stage,
        contexts: &'a IndexSet<BuildContext>,
        out_dir: &'a Utf8Path,
    ) -> LocalBoxFuture<'a, Built> {
        Box::pin(green.real_build_out(containerfile, target, contexts, out_dir))
    }

    fn build_cacheonly<'a>(
        &'a self,
        green: &'a Green,
        containerfile: &'a Utf8Path,
        target: &'a Stage,
    ) -> LocalBoxFuture<'a, Result<()>> {
        Box::pin(green.real_build_cacheonly(containerfile, target))
    }
}
