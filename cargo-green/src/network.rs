//! `RUN`s calling `rustc` and build scripts get no network.
//!
//! This is set on each `RUN` and never with the runner's `--network` flag:
//! * BuildKit hashes the whole exec operation, network mode included, into a step's cache key.
//!   So `RUN --network=none` and `RUN` don't share results.
//! * `RUN --network=none` does share them with a plain `RUN` of a `--network=none` build.
//! * `RUN --network=default` can't undo a build-wide `--network=none`, so that flag is never set.
//!
//! The `RUN`s of base stages (installing the toolchain and OS packages) keep the default network.

pub(crate) const RUN_WITHOUT_NETWORK: &str = "RUN --network=none \\\n";
