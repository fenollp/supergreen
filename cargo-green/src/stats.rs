//! What each wrapped call cost, and what it saved.
//!
//! A build is many processes: cargo spawns one of us per crate, and none of them sees the
//! others. So each writes a line about its own crate to a file the plugin handed down, and
//! the plugin — the one process that outlives the build — sums those lines up into the
//! report at [`Self::report`].
//!
//! Every number here is measured, never guessed: what we cannot time from inside these
//! processes (the runner's own fetching, and one day a networked cache's transfers) is
//! reported as unmeasured rather than as zero.

use std::{
    env,
    fmt::Write as _,
    time::{Duration, Instant},
};

use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, warn};
use serde::{Deserialize, Serialize};

use crate::{PKG, sys::fs};

/// What became of one wrapped `rustc` (or `build.rs`) call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Outcome {
    /// A past build of this very recipe was unpacked instead of being run again.
    Replayed,
    /// The runner built it.
    Built,
    /// No runner, or no recipe to build: the local `rustc` compiled it.
    Compiled,
}

/// One crate's worth of accounting.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Stat {
    pub(crate) krate: String,

    pub(crate) outcome: Option<Outcome>,

    /// Rendering the recipe and hashing it into a results cache key.
    #[serde(default)]
    pub(crate) keying_ms: u64,

    /// Unpacking a replayed result over the target dir.
    #[serde(default)]
    pub(crate) replaying_ms: u64,

    /// The runner, from handing it the recipe to having its artifacts on disk.
    #[serde(default)]
    pub(crate) building_ms: u64,

    /// The local `rustc`, when we fell back to it.
    #[serde(default)]
    pub(crate) compiling_ms: u64,

    /// What the build we replayed had itself taken, when it recorded that.
    #[serde(default)]
    pub(crate) saved_ms: u64,

    /// Result tarball read back (compressed).
    #[serde(default)]
    pub(crate) read_bytes: u64,

    /// Result tarball kept for later (compressed).
    #[serde(default)]
    pub(crate) stored_bytes: u64,

    /// Artifacts landed in the target dir.
    #[serde(default)]
    pub(crate) wrote_bytes: u64,
}

impl Stat {
    pub(crate) fn of(krate: &str) -> Self {
        Self { krate: krate.to_owned(), ..Default::default() }
    }

    /// Appends this to the run's tally, if the plugin asked for one.
    ///
    /// Never fails a build: a report is worth less than the artifacts it describes.
    pub(crate) fn record(&self) {
        let Ok(path) = env::var(CARGOGREEN_STATSPATH!()) else { return };
        let line = match serde_json::to_string(self) {
            Ok(line) => line,
            Err(e) => return warn!("Failed writing stats for {}: {e}", self.krate),
        };
        // One line per append, from as many processes as cargo runs at once.
        if let Err(e) = fs().append(Utf8Path::new(&path), &format!("{line}\n")) {
            warn!("Failed appending stats to {path}: {e}");
        }
    }
}

/// Times a section of work in whole milliseconds.
#[must_use]
pub(crate) fn ms_since(start: Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

/// The whole run, as read back from the tally.
#[derive(Debug, Default)]
pub(crate) struct Report {
    stats: Vec<Stat>,
    /// Size and count of the results cache as it stands after the build.
    cache: Option<(u64, usize)>,
    /// How long the build took, as the one who waited on it saw it.
    wall_ms: u64,
}

impl Report {
    /// Reads the tally the wrapper processes left behind, then forgets it.
    #[must_use]
    pub(crate) fn read(path: &Utf8Path, results: Option<&Utf8Path>, wall_ms: u64) -> Self {
        let stats = match fs().read_to_string(path) {
            Err(e) => {
                debug!("no stats at {path}: {e}");
                return Self::default();
            }
            Ok(txt) => txt
                .lines()
                .filter(|line| !line.is_empty())
                .filter_map(|line| {
                    serde_json::from_str(line)
                        .inspect_err(|e| warn!("Dropping corrupted stats line: {e}"))
                        .ok()
                })
                .collect(),
        };
        let _ = fs().remove_file(path);
        Self { stats, cache: results.and_then(du), wall_ms }
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.stats.is_empty()
    }

    fn count(&self, outcome: Outcome) -> usize {
        self.stats.iter().filter(|s| s.outcome == Some(outcome)).count()
    }

    fn sum(&self, of: impl Fn(&Stat) -> u64) -> u64 {
        self.stats.iter().map(of).sum()
    }

    /// The report, as printed once the build is over.
    #[must_use]
    pub(crate) fn report(&self) -> String {
        let (replayed, built, compiled) = (
            self.count(Outcome::Replayed),
            self.count(Outcome::Built),
            self.count(Outcome::Compiled),
        );

        let mut lines = String::new();
        let _ = writeln!(
            lines,
            "{PKG}: {} crate{} — {replayed} replayed, {built} built, {compiled} compiled locally",
            self.stats.len(),
            if self.stats.len() == 1 { "" } else { "s" },
        );

        // Everything but the wall time is added up over crates that mostly ran side by side.
        let mut times = vec![];
        for (what, ms) in [
            ("wall", self.wall_ms),
            ("keys", self.sum(|s| s.keying_ms)),
            ("replaying", self.sum(|s| s.replaying_ms)),
            ("building", self.sum(|s| s.building_ms)),
            ("locally", self.sum(|s| s.compiling_ms)),
        ] {
            if ms != 0 {
                times.push(format!("{what} {}", secs(ms)));
            }
        }
        if !times.is_empty() {
            let _ = writeln!(
                lines,
                "       time  {} (all but wall are summed over crates)",
                times.join(" · ")
            );
        }

        let saved = self.sum(|s| s.saved_ms);
        if saved != 0 {
            let _ = writeln!(
                lines,
                "      saved  {} of building, for {} of unpacking",
                secs(saved),
                secs(self.sum(|s| s.replaying_ms))
            );
        }

        let mut disk = vec![];
        if let Some((bytes, count)) = self.cache {
            disk.push(format!("{} cached in {count} results", size(bytes)));
        }
        let stored = self.sum(|s| s.stored_bytes);
        if stored != 0 {
            disk.push(format!("{} added", size(stored)));
        }
        let read = self.sum(|s| s.read_bytes);
        if read != 0 {
            disk.push(format!("{} replayed", size(read)));
        }
        let wrote = self.sum(|s| s.wrote_bytes);
        if wrote != 0 {
            disk.push(format!("{} written out", size(wrote)));
        }
        if !disk.is_empty() {
            let _ = writeln!(lines, "       disk  {}", disk.join(" · "));
        }

        // Sources and images are fetched by the runner, inside its own build: those bytes and
        // that wait never cross this process. Neither does a networked cache's, for now.
        let _ = writeln!(lines, "    network  unmeasured: the runner does its own fetching");

        lines
    }
}

/// What the results cache takes up, and how many results that is.
///
/// Reads sizes off the real filesystem: only the plugin ever calls this, once, after the
/// build, and the tests below hand [`Report`] its numbers rather than a directory.
#[must_use]
fn du(dir: &Utf8Path) -> Option<(u64, usize)> {
    let mut bytes = 0;
    let mut count = 0;
    for entry in fs().read_dir(dir).ok()? {
        // Each result is a tarball, plus the small file saying what it cost.
        count += usize::from(entry.as_str().ends_with(".tar.zst"));
        if let Ok(md) = std::fs::metadata(&entry) {
            bytes += md.len();
        }
    }
    Some((bytes, count))
}

#[must_use]
fn secs(ms: u64) -> String {
    let d = Duration::from_millis(ms);
    match d.as_secs() {
        0 => format!("{ms}ms"),
        secs @ 0..60 => format!("{secs}.{:01}s", (ms % 1000) / 100),
        secs => format!("{}m{:02}s", secs / 60, secs % 60),
    }
}

#[must_use]
fn size(bytes: u64) -> String {
    #[expect(clippy::cast_precision_loss)]
    let x = bytes as f64;
    match bytes {
        0..1_024 => format!("{bytes}B"),
        1_024..1_048_576 => format!("{:.0}KiB", x / 1_024.0),
        1_048_576..1_073_741_824 => format!("{:.1}MiB", x / 1_048_576.0),
        _ => format!("{:.2}GiB", x / 1_073_741_824.0),
    }
}

/// Where the plugin has this run's tally written.
#[must_use]
pub(crate) fn path_for(tmp: &Utf8Path, run: &str) -> Utf8PathBuf {
    tmp.join(format!("{PKG}-{run}.stats.jsonl"))
}

#[cfg(test)]
mod tally {
    use snapbox::{assert_data_eq, str};

    use super::{Outcome, Report, Stat, size};

    fn replayed(krate: &str, saved_ms: u64) -> Stat {
        Stat {
            outcome: Some(Outcome::Replayed),
            keying_ms: 3,
            replaying_ms: 40,
            saved_ms,
            read_bytes: 3_000_000,
            wrote_bytes: 9_000_000,
            ..Stat::of(krate)
        }
    }

    #[test]
    fn a_build_that_replayed_everything_says_what_it_saved() {
        let report = Report {
            stats: vec![replayed("anyhow v1.0.100", 4_000), replayed("serde v1.0.228", 32_000)],
            cache: Some((805_306_368, 1469)),
            wall_ms: 1_200,
        };

        assert_data_eq!(
            report.report(),
            str![[r#"
cargo-green: 2 crates — 2 replayed, 0 built, 0 compiled locally
       time  wall 1.2s · keys 6ms · replaying 80ms (all but wall are summed over crates)
      saved  36.0s of building, for 80ms of unpacking
       disk  768.0MiB cached in 1469 results · 5.7MiB replayed · 17.2MiB written out
    network  unmeasured: the runner does its own fetching

"#]]
        );
    }

    #[test]
    fn sizes_read_as_sizes() {
        assert_eq!(size(0), "0B");
        assert_eq!(size(999), "999B");
        assert_eq!(size(1_024), "1KiB");
        assert_eq!(size(1_048_576), "1.0MiB");
        assert_eq!(size(1_073_741_824), "1.00GiB");
    }
}
