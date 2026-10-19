//! In-memory stand-ins for every side effect. Use with `Sys::install()`.

use crate::sys::Sys;

mod builds;
mod cargo;
mod fs;
mod git;
pub(crate) mod http;
pub(crate) mod images;

pub(crate) use builds::FakeBuilds;
pub(crate) use cargo::FakeCargo;
pub(crate) use fs::FakeFs;
pub(crate) use git::FakeGit;
pub(crate) use http::FakeHttp;
pub(crate) use images::FakeImages;

impl Sys {
    #[must_use]
    pub(crate) fn fake() -> Self {
        Self {
            builds: FakeBuilds::new(),
            cargo: FakeCargo::new(),
            fs: FakeFs::new(),
            git: FakeGit::new(),
            http: FakeHttp::new(),
            images: FakeImages::new(),
        }
    }
}
