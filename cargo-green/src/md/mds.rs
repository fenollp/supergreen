// Our own MetaData utils

use std::{collections::HashMap, rc::Rc};

use anyhow::Result;
use camino::{Utf8Path, Utf8PathBuf};

use crate::{
    dirs::{Paths, locate_path},
    md::{Md, MdId},
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

    /// Copy of Paths'
    host_target_dir: Utf8PathBuf,

    cache: HashMap<MdId, Rc<Md>>,
}

impl Paths {
    pub(crate) fn new_mds_cache(&self, path: &Utf8Path) -> Mds {
        Mds {
            target_path: path.to_owned(),
            host_path: self.host_profile_dir(path),
            host_target_dir: self.target_dir().to_owned(),
            cache: HashMap::default(),
        }
    }
}

impl Mds {
    pub(crate) fn load(&mut self, mdid: MdId) -> Result<Rc<Md>> {
        if let Some(md) = self.cache.get(&mdid) {
            return Ok(Rc::clone(md));
        }
        let located =
            locate_path(|path| mdid.path(path), &self.target_path, self.host_path.as_deref());
        let md = Md::from_file(&located, &self.host_target_dir)?;
        let md = Rc::new(md);
        let _ = self.cache.insert(mdid, Rc::clone(&md));
        Ok(md)
    }

    pub(crate) fn load_all(&mut self, mdids: impl Iterator<Item = MdId>) -> Result<Vec<Rc<Md>>> {
        mdids.map(|mdid| self.load(mdid)).collect()
    }
}
