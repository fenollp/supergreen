use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

use crate::stage::Stage;

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct BuildContext {
    pub(crate) name: Stage,
    /// Actually any BuildKit ctx works, we just only use local paths so far.
    pub(crate) uri: Utf8PathBuf,
}
