use std::sync::Arc;

use anyhow::{Result, bail};
use camino::{Utf8Path, Utf8PathBuf};

use crate::sys::Git;

pub(crate) struct FakeGit {
    heads: Vec<(Utf8PathBuf, Utf8PathBuf)>, // Checkout dir --> FETCH_HEAD
}

impl FakeGit {
    #[must_use]
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self { heads: vec![] })
    }

    #[must_use]
    pub(crate) fn with_head(checkout: impl AsRef<Utf8Path>, db: impl AsRef<Utf8Path>) -> Arc<Self> {
        Self::with_heads([(checkout, db)])
    }

    #[must_use]
    pub(crate) fn with_heads<C: AsRef<Utf8Path>, D: AsRef<Utf8Path>>(
        heads: impl IntoIterator<Item = (C, D)>,
    ) -> Arc<Self> {
        Arc::new(Self {
            heads: heads.into_iter().map(|(c, d)| (c.as_ref().into(), d.as_ref().into())).collect(),
        })
    }
}

impl Git for FakeGit {
    fn fetch_head(&self, pkg_manifest_dir: &Utf8Path) -> Result<Utf8PathBuf> {
        match self.heads.iter().find(|(dir, _)| dir == pkg_manifest_dir) {
            Some((_, head)) => Ok(head.clone()),
            None => bail!("no fake git repository for {pkg_manifest_dir}"),
        }
    }
}
