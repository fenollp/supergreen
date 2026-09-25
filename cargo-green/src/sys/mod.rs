//! Injectable side effects
//!
//! Usualy, use Sys-returning fns. Tests have to `install()` on each thread or this crashes, so
//! they don't step on each others' toes.
//!
//! Test with `Builder::new_current_thread` instead of multi (and no spawn) or this will
//! panic (rather than quietly falling back to touching the real FS, network, ...).

mod builds;
mod fs;
mod git;

#[cfg(test)]
pub(crate) mod fake;
pub(crate) mod real;

pub(crate) use builds::Builds;
pub(crate) use fs::Fs;
pub(crate) use git::Git;

#[cfg(not(test))]
static REAL: std::sync::LazyLock<Sys> = std::sync::LazyLock::new(Sys::real);

#[derive(Clone)]
pub(crate) struct Sys {
    pub(crate) builds: SysBuilds,
    pub(crate) fs: SysFs,
    pub(crate) git: SysGit,
}

macro_rules! device {
    ($systype:ident, $trait:ident, $dev:ident) => {
        pub(crate) type $systype = std::sync::Arc<dyn $trait>;

        #[must_use]
        pub(crate) fn $dev() -> $systype {
            sys().$dev
        }
    };
}

device!(SysBuilds, Builds, builds);
device!(SysFs, Fs, fs);
device!(SysGit, Git, git);

#[must_use]
fn sys() -> Sys {
    #[cfg(not(test))]
    {
        REAL.clone()
    }
    #[cfg(test)]
    {
        crate::sys::mutable::PIVOT
            .with_borrow(Clone::clone)
            .expect("BUG: reached a side effect with no Sys installed in current thread")
    }
}

#[cfg(test)]
mod mutable {
    use std::cell::RefCell;

    use super::Sys;

    thread_local! {
        /// What `install()` put in place, for the duration of one test, on its own thread.
        pub(super) static PIVOT: RefCell<Option<Sys>> = const { RefCell::new(None) };
    }

    impl Sys {
        #[must_use]
        pub(crate) fn install(sub: Self) -> Guard {
            Guard { previous: PIVOT.with_borrow_mut(|slot| slot.replace(sub)) }
        }
    }

    /// Restores what was in place before, so one test cannot leak into the next.
    pub(crate) struct Guard {
        previous: Option<Sys>,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            PIVOT.with_borrow_mut(|slot| *slot = self.previous.take());
        }
    }
}

#[cfg(test)]
mod isolation {
    use std::sync::Arc;

    use super::{Sys, sys};

    #[test]
    fn real_side_effects_can_be_asked_for_explicitly() {
        let _guard = Sys::install(Sys::real());
        assert!(!sys().fs.exists("/definitely/not/a/real/path".into()));
    }

    #[test]
    fn nothing_is_in_force_by_default_when_testing() {
        assert!(std::panic::catch_unwind(sys).is_err());
    }

    #[test]
    fn a_guard_restores_what_it_replaced() {
        let outer = Sys::fake();
        let outer_fs = Arc::as_ptr(&outer.fs);
        let guard = Sys::install(outer);
        assert_eq!(Arc::as_ptr(&sys().fs), outer_fs);

        {
            let inner = Sys::fake();
            let inner_fs = Arc::as_ptr(&inner.fs);
            let _inner_guard = Sys::install(inner);
            assert_eq!(Arc::as_ptr(&sys().fs), inner_fs);
        }

        assert_eq!(Arc::as_ptr(&sys().fs), outer_fs, "inner guard restored the outer");
        drop(guard);
        assert!(std::panic::catch_unwind(sys).is_err(), "outer guard restored the absence");
    }

    #[test]
    fn a_fake_does_not_follow_a_spawned_task() {
        let _guard = Sys::install(Sys::fake());

        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).build().unwrap();
        let joined = rt.block_on(async {
            let _ = sys();
            tokio::spawn(async { drop(sys()) }).await
        });

        assert!(joined.unwrap_err().is_panic());
    }
}
