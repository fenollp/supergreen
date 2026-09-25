use anyhow::Result;
use camino::{Utf8Path, Utf8PathBuf};

/// Git repository operations.
pub(crate) trait Git: Send + Sync {
    /// Locate the `FETCH_HEAD` of the git db backing `pkg_manifest_dir`'s checkout.
    ///
    /// e.g. `$CARGO_HOME/git/db/remarkable-tools-9f4e9942cc4e93a3/FETCH_HEAD`
    fn fetch_head(&self, pkg_manifest_dir: &Utf8Path) -> Result<Utf8PathBuf>;
}
