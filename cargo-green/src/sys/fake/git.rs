use std::sync::Arc;

use anyhow::{Result, bail};
use camino::{Utf8Path, Utf8PathBuf};

use crate::sys::Git;

pub(crate) struct FakeGit {
    heads: Option<(Utf8PathBuf, Utf8PathBuf)>, // Checkout dir --> FETCH_HEAD
}

impl FakeGit {
    #[must_use]
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self { heads: None })
    }

    #[must_use]
    pub(crate) fn with_head(checkout: impl AsRef<Utf8Path>, db: impl AsRef<Utf8Path>) -> Arc<Self> {
        Arc::new(Self { heads: Some((checkout.as_ref().into(), db.as_ref().into())) })
    }
}

impl Git for FakeGit {
    fn fetch_head(&self, pkg_manifest_dir: &Utf8Path) -> Result<Utf8PathBuf> {
        match &self.heads {
            Some((dir, head)) if dir == pkg_manifest_dir => Ok(head.clone()),
            _ => bail!("no fake git repository for {pkg_manifest_dir}"),
        }
    }
}
