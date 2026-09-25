//! The side effects actually affecting the real world.

use std::sync::Arc;

use crate::sys::Sys;

mod builds;
mod fs;

pub(crate) use builds::RealBuilds;
pub(crate) use fs::RealFs;

impl Sys {
    #[must_use]
    pub(crate) fn real() -> Self {
        Self { builds: Arc::new(RealBuilds {}), fs: Arc::new(RealFs {}) }
    }
}
