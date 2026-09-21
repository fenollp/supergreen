//! The side effects actually affecting the real world.

use std::sync::Arc;

use crate::sys::Sys;

mod fs;

pub(crate) use fs::RealFs;

impl Sys {
    #[must_use]
    pub(crate) fn real() -> Self {
        Self { fs: Arc::new(RealFs) }
    }
}
