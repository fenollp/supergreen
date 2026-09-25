//! In-memory stand-ins for every side effect. Use with `Sys::install()`.

use crate::sys::Sys;

mod builds;
mod fs;
mod git;

pub(crate) use builds::FakeBuilds;
pub(crate) use fs::FakeFs;
pub(crate) use git::FakeGit;

impl Sys {
    #[must_use]
    pub(crate) fn fake() -> Self {
        Self { builds: FakeBuilds::new(), fs: FakeFs::new(), git: FakeGit::new() }
    }
}
