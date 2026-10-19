//! The side effects actually affecting the real world.

use std::sync::Arc;

use crate::sys::Sys;

mod builds;
mod cargo;
mod fs;
mod git;
mod http;
mod images;

pub(crate) use builds::RealBuilds;
pub(crate) use cargo::RealCargo;
pub(crate) use fs::RealFs;
pub(crate) use git::RealGit;
pub(crate) use http::RealHttp;
pub(crate) use images::RealImages;

impl Sys {
    #[must_use]
    pub(crate) fn real() -> Self {
        Self {
            builds: Arc::new(RealBuilds {}),
            cargo: Arc::new(RealCargo {}),
            fs: Arc::new(RealFs {}),
            git: Arc::new(RealGit {}),
            http: Arc::new(RealHttp {}),
            images: Arc::new(RealImages {}),
        }
    }
}
