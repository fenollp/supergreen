//! Injectable side effects: filesystem, git discovery, image digests, runner builds.
//!
//! Production code reaches these through [`sys()`], which always yields the real ones.
//! Tests swap the whole bundle with install(), which restores the previous one when
//! its guard drops.
//!
//! The override is a `thread_local`, so two tests installing different fakes cannot see
//! each other's and the suite needs no serialisation — under `cargo test`'s thread pool
//! as much as under `cargo nextest`'s process-per-test. The cost is that a fake is only
//! visible on the thread that installed it: tests must therefore drive async code with
//! a **current-thread** runtime (`Builder::new_current_thread`), never a multi-threaded
//! one, and must not `tokio::spawn`. Getting that wrong is not silent — [`sys()`] panics
//! rather than quietly falling back to touching the real filesystem, network or runner.

#[cfg(test)]
use std::cell::RefCell;
use std::sync::Arc;
#[cfg(not(test))]
use std::sync::LazyLock;

mod builds;
mod fs;
mod git;
mod images;

#[cfg(test)]
pub(crate) mod fake;

pub(crate) use builds::*;
pub(crate) use fs::*;
pub(crate) use git::*;
pub(crate) use images::*;

#[cfg(not(test))]
static REAL: LazyLock<Sys> = LazyLock::new(Sys::real);

#[cfg(test)]
thread_local! {
    /// What [`install`] put in place, for the duration of one test, on its own thread.
    static OVERRIDE: RefCell<Option<Sys>> = const { RefCell::new(None) };
}

#[derive(Clone)]
pub(crate) struct Sys {
    pub(crate) fs: Arc<dyn Fs>,
    pub(crate) git: Arc<dyn Git>,
    pub(crate) images: Arc<dyn Images>,
    pub(crate) builds: Arc<dyn Builds>,
}

impl Sys {
    /// The side effects that actually touch the world.
    #[must_use]
    pub(crate) fn real() -> Self {
        Self {
            fs: Arc::new(RealFs),
            git: Arc::new(RealGit),
            images: Arc::new(RealImages),
            builds: Arc::new(RealBuilds),
        }
    }
}

/// The side effects in force. Cheap: clones four [`Arc`]s.
#[cfg(not(test))]
#[must_use]
pub(crate) fn sys() -> Sys {
    REAL.clone()
}

/// Under test, only ever what [`install`] put in place on this thread.
///
/// Panicking here is deliberate. A test that reaches a side effect without installing
/// one would otherwise read the developer's real `$CARGO_HOME`, or shell out to Docker,
/// and pass or fail depending on the machine it ran on.
#[cfg(test)]
#[must_use]
pub(crate) fn sys() -> Sys {
    OVERRIDE.with_borrow(Clone::clone).expect(
        "BUG: reached a side effect with no Sys installed. \
         Either install one (`let _guard = sys::install(Sys { .. ..Sys::fake() });`, or \
         `sys::install(Sys::real())` to opt into real IO), or, if a fake *was* installed, \
         this ran off the installing thread: use Builder::new_current_thread and do not spawn.",
    )
}

/// Puts `fake` in force on this thread until the returned guard drops.
#[cfg(test)]
#[must_use]
pub(crate) fn install(fake: Sys) -> Guard {
    Guard { previous: OVERRIDE.with_borrow_mut(|slot| slot.replace(fake)) }
}

/// Restores what was in force before, so one test cannot leak into the next.
#[cfg(test)]
pub(crate) struct Guard {
    previous: Option<Sys>,
}

#[cfg(test)]
impl Drop for Guard {
    fn drop(&mut self) {
        OVERRIDE.with_borrow_mut(|slot| *slot = self.previous.take());
    }
}

#[cfg(test)]
mod isolation {
    use super::{Sys, install, sys};

    #[test]
    fn nothing_is_in_force_by_default() {
        assert!(std::panic::catch_unwind(sys).is_err());
    }

    #[test]
    fn a_guard_restores_what_it_replaced() {
        let outer = Sys::fake();
        let outer_fs = std::sync::Arc::as_ptr(&outer.fs);
        let guard = install(outer);
        assert_eq!(std::sync::Arc::as_ptr(&sys().fs), outer_fs);

        {
            let inner = Sys::fake();
            let inner_fs = std::sync::Arc::as_ptr(&inner.fs);
            let _inner_guard = install(inner);
            assert_eq!(std::sync::Arc::as_ptr(&sys().fs), inner_fs);
        }

        assert_eq!(std::sync::Arc::as_ptr(&sys().fs), outer_fs, "inner guard restored the outer");
        drop(guard);
        assert!(std::panic::catch_unwind(sys).is_err(), "outer guard restored the absence");
    }

    /// The escape hatch for a test that genuinely wants to touch the real world.
    #[test]
    fn real_side_effects_can_be_asked_for_explicitly() {
        let _guard = install(Sys::real());
        assert!(!sys().fs.exists("/definitely/not/a/real/path".into()));
    }

    /// The one way to misuse this: a fake does not follow work onto another thread.
    /// `block_on` runs on the caller, so a current-thread runtime is fine; a task
    /// spawned onto a worker is not, and says so instead of reading the real world.
    #[test]
    fn a_fake_does_not_follow_a_spawned_task() {
        let _guard = install(Sys::fake());

        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).build().unwrap();
        let joined = rt.block_on(async {
            // Same thread as the installer: in force.
            let _ = sys();
            tokio::spawn(async { drop(sys()) }).await
        });

        assert!(joined.unwrap_err().is_panic(), "should have panicked off-thread");
    }
}
