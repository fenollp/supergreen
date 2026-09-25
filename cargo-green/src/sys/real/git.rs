use anyhow::Result;
use camino::{Utf8Path, Utf8PathBuf};

use crate::sys::Git;

pub(crate) struct RealGit;

impl Git for RealGit {
    fn fetch_head(&self, pkg_manifest_dir: &Utf8Path) -> Result<Utf8PathBuf> {
        crate::checkouts::real_fetch_head(pkg_manifest_dir)
    }
}
