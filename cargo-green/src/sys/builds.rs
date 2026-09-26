use anyhow::Result;
use camino::Utf8Path;
use futures::future::LocalBoxFuture;
use indexmap::IndexSet;

use crate::{build::Built, green::Green, md::BuildContext, stage::Stage};

/// Runnner build operations.
pub(crate) trait Builds: Send + Sync {
    /// Build `target` and export its outputs into `out_dir`.
    #[must_use]
    fn build_out<'a>(
        &'a self,
        green: &'a Green,
        containerfile: &'a Utf8Path,
        target: &'a Stage,
        contexts: &'a IndexSet<BuildContext>,
        out_dir: &'a Utf8Path,
    ) -> LocalBoxFuture<'a, Built>;

    /// Build `target` for its cache effect only, exporting nothing.
    #[must_use]
    fn build_cacheonly<'a>(
        &'a self,
        green: &'a Green,
        containerfile: &'a Utf8Path,
        target: &'a Stage,
    ) -> LocalBoxFuture<'a, Result<()>>;
}
