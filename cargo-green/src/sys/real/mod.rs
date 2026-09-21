//! The side effects actually affecting the real world.

use crate::sys::Sys;

impl Sys {
    #[expect(dead_code)]
    #[must_use]
    pub(crate) fn real() -> Self {
        Self {}
    }
}
