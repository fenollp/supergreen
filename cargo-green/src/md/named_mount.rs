use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

use crate::stage::Stage;

#[derive(Debug, Clone, Deserialize, Serialize, Eq)]
pub(crate) struct NamedMount {
    pub(crate) name: Stage,
    pub(crate) mount: Utf8PathBuf,
}

/// For use by IndexSet
impl PartialEq for NamedMount {
    fn eq(&self, other: &Self) -> bool {
        self.mount == other.mount
    }
}

/// For use by IndexSet
impl std::hash::Hash for NamedMount {
    fn hash<H>(&self, state: &mut H)
    where
        H: std::hash::Hasher,
    {
        self.mount.hash(state);
    }
}
