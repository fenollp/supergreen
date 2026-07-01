// Our own MetaData utils

use std::{collections::HashMap, rc::Rc};

use anyhow::Result;
use camino::{Utf8Path, Utf8PathBuf};

use crate::{
    md::{Md, MdId},
    target_dir::host_profile_dir,
};

/// A file cache
#[derive(Debug)]
pub(crate) struct Mds {
    target_path: Utf8PathBuf,

    /// When cross-compiling (`--target=TARGET`) `target_path` contains `TARGET` but ALSO
    /// stores host-specific artifacts under non-TARGET'ed `target_path` (proc-macros, build scripts and their results).
    /// So at that point both `$CARGO_TARGET_DIR/$PROFILE` and `$CARGO_TARGET_DIR/<TARGET>/$PROFILE` coexist.
    /// This is `None` when not given `--target`.
    host_path: Option<Utf8PathBuf>,

    cache: HashMap<MdId, Rc<Md>>,
}

impl Mds {
    pub(crate) fn new(path: &Utf8Path) -> Self {
        Self {
            target_path: path.to_owned(),
            host_path: host_profile_dir(path),
            cache: HashMap::default(),
        }
    }

    pub(crate) fn load(&mut self, mdid: MdId) -> Result<Rc<Md>> {
        if let Some(md) = self.cache.get(&mdid) {
            return Ok(Rc::clone(md));
        }
        let md = Md::from_file(&self.locate(mdid))?;
        let md = Rc::new(md);
        let _ = self.cache.insert(mdid, Rc::clone(&md));
        Ok(md)
    }

    //FIXME: devise the logic and avoid fs checks
    //  FIXME true that proc-macros and build scripts always live in host never in triple-target-path?
    #[must_use]
    fn locate(&self, mdid: MdId) -> Utf8PathBuf {
        let primary = mdid.path(&self.target_path);
        if primary.exists() {
            return primary;
        }
        if let Some(host_path) = &self.host_path {
            let host = mdid.path(host_path);
            if host.exists() {
                return host;
            }
        }
        primary // Keep primary so `Md::from_file` emits its helpful not-found message
    }

    pub(crate) fn load_all(&mut self, mdids: impl Iterator<Item = MdId>) -> Result<Vec<Rc<Md>>> {
        mdids.map(|mdid| self.load(mdid)).collect()
    }
}
