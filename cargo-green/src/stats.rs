//! What each wrapped call cost.
//!
//! A build is many processes: cargo spawns one of us per crate, and none of them sees the
//! others. A build tells its [`BuildStats`] to the plugin (over [`crate::chitchat`]), which keeps
//! them next to the results cache: that is how replays know what they saved. A replay
//! keeps its [`ReplayStats`] in the crate's `Md`, in the target dir.
//!
//! Every number here is measured, never guessed.

use std::time::Instant;

use serde::{Deserialize, Serialize};

/// What building a crate cost: describes that one build, and is kept for as long as its result.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct BuildStats {
    /// Built on a remote daemon (`ssh://` or `tcp://`, reached through our connection pool).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) remote: bool,

    /// The runner, from handing it the recipe to having its artifacts on disk.
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) building_ms: u64,
}

/// What replaying a stored result cost: only ever in the target dir, for this run's report.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ReplayStats {
    /// Unpacking the result over the target dir.
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) replaying_ms: u64,

    /// Result tarball read back (compressed).
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) read_bytes: u64,
}

#[expect(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if hands a ref
fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// Times a section of work in whole milliseconds.
#[must_use]
pub(crate) fn ms_since(start: Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}
