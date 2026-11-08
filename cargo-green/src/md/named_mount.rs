use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

use crate::stage::Stage;

#[derive(Debug, Clone, Deserialize, Serialize, Eq)]
pub(crate) struct NamedMount {
    pub(crate) name: Stage,
    pub(crate) mount: Utf8PathBuf,
}

/// For use by BTreeSet
impl PartialEq for NamedMount {
    fn eq(&self, other: &Self) -> bool {
        self.mount == other.mount
    }
}

/// For use by BTreeSet
impl Ord for NamedMount {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.mount.cmp(&other.mount)
    }
}

/// For use by BTreeSet
impl PartialOrd for NamedMount {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
