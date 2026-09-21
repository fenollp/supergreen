//! In-memory stand-ins for every side effect. Use with `Sys::install()`.

use crate::sys::Sys;

impl Sys {
    #[must_use]
    pub(crate) fn fake() -> Self {
        Self {}
    }
}
