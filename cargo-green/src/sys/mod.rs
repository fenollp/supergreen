//! Injectable side effects
//!
//! Usualy, use Sys-returning fns. Tests have to `install()` on each thread or this crashes, so
//! they don't step on each others' toes.
//!
//! Test with `Builder::new_current_thread` instead of multi (and no spawn) or this will
//! panic (rather than quietly falling back to touching the real FS, network, ...).

#[cfg(test)]
pub(crate) mod fake;
pub(crate) mod real;

#[expect(dead_code)]
static REAL: std::sync::LazyLock<Sys> = std::sync::LazyLock::new(Sys::real);

#[derive(Clone)]
pub(crate) struct Sys {}

#[cfg_attr(not(test), expect(dead_code))]
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
    use super::{Sys, sys};

    #[test]
    fn nothing_is_in_force_by_default_when_testing() {
        assert!(std::panic::catch_unwind(sys).is_err());
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
