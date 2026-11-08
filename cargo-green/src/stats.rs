//! What each wrapped call cost.
//!
//! A build is many processes. Wrappers tell the plugin, over [`crate::chitchat`],
//! what their crate cost: its [`BuildStats`] when built, its [`ReplayStats`]
//! (and the `BuildStats` kept next to the results cache, from when it was built) when replayed.
//! The plugin — the one process that outlives the build — sums it all up into the report
//! at [`Report::report`]. None of it is in `Md`s nor in results: those stay deterministic.

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

/// What replaying a stored result cost: only told to the plugin, for this run's report.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub(crate) struct ReplayStats {
    /// Unpacking the result over the target dir.
    pub(crate) replaying_ms: u64,

    /// Result tarball read back (compressed).
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
