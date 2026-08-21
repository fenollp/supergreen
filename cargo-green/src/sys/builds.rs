use anyhow::Result;
use camino::Utf8Path;
use futures::future::LocalBoxFuture;
use indexmap::IndexSet;

use crate::{
    build::Effects, cache::result::ResultWriter, green::Green, md::BuildContext, stage::Stage,
};

/// Invocation of the runner (`docker buildx build` / `podman build`).
///
/// This is the seam that lets the generation path be exercised without a BuildKit
/// daemon: everything downstream of it (tar extraction, result caching, artifact
/// export) is reached only through a real build.
pub(crate) trait Builds: Send + Sync {
    /// Build `target` and export its outputs into `out_dir`.
    ///
    /// Returns the call and env strings that were used (for `$CARGOGREEN_FINAL_PATH`),
    /// the build's [`Effects`], a writer for the result tarball, and the build outcome.
    #[expect(clippy::type_complexity)]
    fn build_out<'a>(
        &'a self,
        green: &'a Green,
        containerfile: &'a Utf8Path,
        target: &'a Stage,
        contexts: &'a IndexSet<BuildContext>,
        out_dir: &'a Utf8Path,
    ) -> LocalBoxFuture<'a, (String, String, Effects, Option<ResultWriter>, Result<()>)>;

    /// Build `target` for its cache effect only, exporting nothing.
    fn build_cacheonly<'a>(
        &'a self,
        green: &'a Green,
        containerfile: &'a Utf8Path,
        target: &'a Stage,
    ) -> LocalBoxFuture<'a, Result<()>>;
}

pub(crate) struct RealBuilds;

impl Builds for RealBuilds {
    fn build_out<'a>(
        &'a self,
        green: &'a Green,
        containerfile: &'a Utf8Path,
        target: &'a Stage,
        contexts: &'a IndexSet<BuildContext>,
        out_dir: &'a Utf8Path,
    ) -> LocalBoxFuture<'a, (String, String, Effects, Option<ResultWriter>, Result<()>)> {
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
