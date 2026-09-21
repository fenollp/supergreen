//! In-memory stand-ins for every side effect. Use with `Sys::install()`.

use crate::sys::Sys;

mod fs;

pub(crate) use fs::FakeFs;

impl Sys {
    #[must_use]
    pub(crate) fn fake() -> Self {
        Self { fs: FakeFs::new() }
    }
}
