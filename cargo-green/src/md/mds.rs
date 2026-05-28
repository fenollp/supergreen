// Our own MetaData utils

use std::{collections::HashMap, rc::Rc};

use anyhow::Result;
use camino::{Utf8Path, Utf8PathBuf};

use crate::md::{Md, MdId};

/// A file cache
#[derive(Debug)]
pub(crate) struct Mds {
    target_path: Utf8PathBuf,
    cache: HashMap<MdId, Rc<Md>>,
}

impl Mds {
    pub(crate) fn new(path: &Utf8Path) -> Self {
        Self { target_path: path.to_owned(), cache: HashMap::default() }
    }

    pub(crate) fn load(&mut self, mdid: MdId) -> Result<Rc<Md>> {
        if let Some(md) = self.cache.get(&mdid) {
            return Ok(Rc::clone(md));
        }
        let md = Md::from_file(&mdid.path(&self.target_path))?;
        let md = Rc::new(md);
        let _ = self.cache.insert(mdid, Rc::clone(&md));
        Ok(md)
    }

    pub(crate) fn load_all(&mut self, mdids: impl Iterator<Item = MdId>) -> Result<Vec<Rc<Md>>> {
        mdids.map(|mdid| self.load(mdid)).collect()
    }
}
