//! Injectable side effects: filesystem, git discovery, image digests, runner builds.
//!
//! Production code reaches these through [`sys()`]. Tests swap the whole bundle with
//! [`install`], which restores the previous one when its guard drops.
//!
//! The bundle is process-global rather than threaded through call sites, and read
//! through an [`RwLock`] rather than a `thread_local`, because the wrapping pipeline
//! runs on a multi-thread tokio runtime: an override installed by the test thread has
//! to be visible from the worker threads that actually run `wrap_rustc`.

use std::sync::{Arc, LazyLock, PoisonError, RwLock};

mod builds;
mod fs;
mod git;
mod images;

pub(crate) use builds::*;
pub(crate) use fs::*;
pub(crate) use git::*;
pub(crate) use images::*;

#[cfg(test)]
pub(crate) mod fake;

#[derive(Clone)]
pub(crate) struct Sys {
    pub(crate) fs: Arc<dyn Fs>,
    pub(crate) git: Arc<dyn Git>,
    pub(crate) images: Arc<dyn Images>,
    pub(crate) builds: Arc<dyn Builds>,
}

impl Sys {
    #[must_use]
    fn real() -> Self {
        Self {
            fs: Arc::new(RealFs),
            git: Arc::new(RealGit),
            images: Arc::new(RealImages),
            builds: Arc::new(RealBuilds),
        }
    }
}

static CURRENT: LazyLock<RwLock<Sys>> = LazyLock::new(|| RwLock::new(Sys::real()));

/// The side effects in force. Cheap: clones four [`Arc`]s.
#[must_use]
pub(crate) fn sys() -> Sys {
    CURRENT.read().unwrap_or_else(PoisonError::into_inner).clone()
}

#[cfg(test)]
use std::sync::{Mutex, MutexGuard};

/// Serialises tests that install fakes, since the bundle is process-global.
#[cfg(test)]
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// Restores the previous [`Sys`] on drop. Hold it for the duration of the test.
#[cfg(test)]
pub(crate) struct Guard {
    // A test that panics mid-way poisons this; recovery is intentional, as the panic is
    // the failure being reported and a poisoned lock would bury it.
    _one_at_a_time: MutexGuard<'static, ()>,
    previous: Sys,
}

#[cfg(test)]
impl Drop for Guard {
    fn drop(&mut self) {
        *CURRENT.write().unwrap_or_else(PoisonError::into_inner) = self.previous.clone();
    }
}

#[cfg(test)]
#[must_use]
pub(crate) fn install(fake: Sys) -> Guard {
    let one_at_a_time = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    let mut current = CURRENT.write().unwrap_or_else(PoisonError::into_inner);
    let previous = current.clone();
    *current = fake;
    drop(current);
    Guard { _one_at_a_time: one_at_a_time, previous }
}
