//! What each wrapped call cost, and what it saved.
//!
//! A build is many processes. Wrappers tell the plugin, over [`crate::chitchat`],
//! what their crate cost: its [`BuildStats`] when built, its [`ReplayStats`]
//! (and the `BuildStats` kept next to the results cache, from when it was built) when replayed.
//! The plugin — the one process that outlives the build — sums it all up into the report
//! at [`Report::report`]. None of it is in `Md`s nor in results: those stay deterministic.

use std::{fmt::Write, time::Instant};

use serde::{Deserialize, Serialize};

use crate::{PKG, coloring::Paint};

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

/// The whole run, as its wrappers told it.
#[derive(Debug, Default)]
pub(crate) struct Report {
    /// Crates this run built.
    built: Vec<BuildStats>,
    /// Crates this run replayed, with what building them had cost (when that was recorded).
    replayed: Vec<(ReplayStats, BuildStats)>,
    /// Results this run stored in the results cache: new ones
    stored: usize,
    /// How long the build took, as the one who waited on it saw it.
    wall_ms: u64,
}

impl Report {
    #[must_use]
    pub(crate) const fn new(
        built: Vec<BuildStats>,
        replayed: Vec<(ReplayStats, BuildStats)>,
        stored: usize,
        wall_ms: u64,
    ) -> Self {
        Self { built, replayed, stored, wall_ms }
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.built.is_empty() && self.replayed.is_empty()
    }

    /// The report, as printed once the build is over, in the colors of `paint`: two lines.
    ///
    /// What was saved is what replayed builds had taken, minus what replaying them took. It is
    /// a total over crates that mostly ran side by side, not a wall-clock claim: so it is told
    /// as a share of what building everything would have taken (all the same sums).
    #[must_use]
    pub(crate) fn report(&self, paint: &Paint) -> String {
        let (green, yellow, cyan) = (&paint.ok, &paint.skip, &paint.note);
        // Cargo's own status style: bold green labels, right-aligned on 12 columns.
        let label = |txt: &str| paint.title.apply_to(format!("{txt:>12}"));

        let (replayed, built) = (self.replayed.len(), self.built.len());
        let total = replayed + built;
        let remotely = self.built.iter().filter(|b| b.remote).count();

        let mut counts = vec![];
        if replayed != 0 {
            let share = percent(count(replayed), count(total));
            counts.push(green.apply_to(format!("{replayed} replayed ({share}%)")).to_string());
        }
        if built != 0 {
            counts.push(yellow.apply_to(format!("{built} built")).to_string());
        }
        if remotely != 0 {
            counts.push(cyan.apply_to(format!("{remotely} remotely")).to_string());
        }
        if self.stored != 0 {
            counts.push(green.apply_to(format!("{} new", self.stored)).to_string());
        }
        let mut lines = format!(
            "{} {total} crate{} — {}\n",
            paint.title.apply_to(format!("{PKG}:")),
            if total == 1 { "" } else { "s" },
            counts.join(", "),
        );

        let replaying: u64 = self.replayed.iter().map(|(r, _)| r.replaying_ms).sum();
        let building: u64 = self.built.iter().map(|b| b.building_ms).sum();
        let was: u64 = self.replayed.iter().map(|(_, b)| b.building_ms).sum();
        let saved = was.saturating_sub(replaying);

        let mut times = vec![];
        if saved != 0 {
            let share = percent(saved, was + building);
            times.push(green.apply_to(format!("saved {} ({share}%)", secs(saved))).to_string());
        }
        for (what, ms) in [("wall", self.wall_ms), ("total", replaying), ("building", building)] {
            if ms != 0 {
                times.push(format!("{what} {}", secs(ms)));
            }
        }
        // Compressed tarballs, as stored in the results cache
        let read: u64 = self.replayed.iter().map(|(r, _)| r.read_bytes).sum();
        if read != 0 {
            times.push(format!("{} read back", human_size(read.div_ceil(1_024))));
        }
        if !times.is_empty() {
            let _ = writeln!(lines, "{}  {}", label("time"), times.join(" · "));
        }

        lines
    }
}

/// Whole percents of `part` in `whole`, rounded down (and 0 when there is no `whole`)
#[must_use]
fn percent(part: u64, whole: u64) -> u64 {
    (part.saturating_mul(100)).checked_div(whole).unwrap_or_default().min(100)
}

/// Like `du -h`, with binary prefixes: `12.0KiB`, `2.0MiB`. Takes KiB, as `du` reports them.
#[must_use]
pub(crate) fn human_size(kib: u64) -> String {
    if kib == 0 {
        return "0".to_owned();
    }
    let units = ["KiB", "MiB", "GiB", "TiB", "PiB"];
    let (mut whole, mut rest, mut idx) = (kib, 0, 0);
    while whole >= 1_024 && idx + 1 < units.len() {
        (whole, rest, idx) = (whole / 1_024, whole % 1_024, idx + 1);
    }
    // One decimal, rounded
    let mut tenths = (rest * 10 + 512) / 1_024;
    if tenths == 10 {
        (whole, tenths) = (whole + 1, 0);
    }
    format!("{whole}.{tenths}{}", units[idx])
}

fn count(n: usize) -> u64 {
    n.try_into().unwrap_or(u64::MAX)
}

/// `830ms`, `5.23s`, `13m`, `5m9s`, `1h2m`
#[must_use]
fn secs(ms: u64) -> String {
    match ms {
        0..1_000 => format!("{ms}ms"),
        1_000..60_000 => format!("{}.{:02}s", ms / 1_000, (ms % 1_000) / 10),
        _ => {
            let s = ms / 1_000;
            let mut out = String::new();
            for (n, unit) in [(s / 3_600, 'h'), (s / 60 % 60, 'm'), (s % 60, 's')] {
                if n != 0 {
                    let _ = write!(out, "{n}{unit}");
                }
            }
            out
        }
    }
}

#[cfg(test)]
mod report {
    use snapbox::{assert_data_eq, str};

    use super::{BuildStats, ReplayStats, Report, human_size, secs};
    use crate::coloring::Paint;

    fn replayed(building_ms: u64) -> (ReplayStats, BuildStats) {
        (
            ReplayStats { replaying_ms: 40, read_bytes: 3_000_000 },
            BuildStats { building_ms, ..BuildStats::default() },
        )
    }

    fn built(remote: bool, building_ms: u64) -> BuildStats {
        BuildStats { remote, building_ms }
    }

    #[test]
    fn a_build_that_replayed_everything_says_what_it_saved() {
        let report = Report {
            replayed: vec![replayed(4_000), replayed(32_000), replayed(20_000)],
            wall_ms: 1_200,
            ..Report::default()
        };

        assert_data_eq!(
            report.report(&Paint::plain(80)),
            str![[r#"
cargo-green: 3 crates — 3 replayed (100%)
        time  saved 55.88s (99%) · wall 1.20s · total 120ms · 8.6MiB read back

"#]]
        );
    }

    #[test]
    fn a_build_says_what_it_built() {
        let report = Report {
            built: vec![built(true, 92_000), built(false, 8_000)],
            stored: 2,
            wall_ms: 94_000,
            ..Report::default()
        };

        assert_data_eq!(
            report.report(&Paint::plain(80)),
            str![[r#"
cargo-green: 2 crates — 2 built, 1 remotely, 2 new
        time  wall 1m34s · building 1m40s

"#]]
        );
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human_size(0), "0");
        assert_eq!(human_size(12), "12.0KiB");
        assert_eq!(human_size(2048), "2.0MiB");
        assert_eq!(human_size(5 * 1024 * 1024 * 1024), "5.0TiB");
    }

    #[test]
    fn times_read_as_times() {
        assert_eq!(secs(0), "0ms");
        assert_eq!(secs(830), "830ms");
        assert_eq!(secs(5_234), "5.23s");
        assert_eq!(secs(59_999), "59.99s");
        assert_eq!(secs(60_000), "1m");
        assert_eq!(secs(309_000), "5m9s");
        assert_eq!(secs(780_000), "13m");
        assert_eq!(secs(3_725_000), "1h2m5s");
        assert_eq!(secs(3_600_000), "1h");
    }

    #[test]
    fn a_mixed_build_tells_shares_not_wall_clock_claims() {
        let report = Report {
            replayed: vec![replayed(6_000)],
            built: vec![built(true, 2_000), built(false, 2_000)],
            stored: 1,
            wall_ms: 1_200,
        };
        assert_data_eq!(
            report.report(&Paint::plain(80)),
            str![[r#"
cargo-green: 3 crates — 1 replayed (33%), 2 built, 1 remotely, 1 new
        time  saved 5.96s (59%) · wall 1.20s · total 40ms · building 4.00s · 2.9MiB read back

"#]]
        );
    }

    #[test]
    fn colors_only_wrap_the_text() {
        let report =
            Report { replayed: vec![replayed(4_000)], wall_ms: 1_200, ..Report::default() };
        let colored = report.report(&Paint::for_stderr(true));
        let plain = report.report(&Paint::plain(80));
        assert!(colored.starts_with('\x1b'), "{colored:?}");
        assert_ne!(colored, plain);
        // Colors only wrap text: strip them and there is the plain report
        let mut stripped = String::new();
        let mut in_escape = false;
        for c in colored.chars() {
            match (in_escape, c) {
                (_, '\x1b') => in_escape = true,
                (true, 'm') => in_escape = false,
                (false, c) => stripped.push(c),
                (true, _) => {}
            }
        }
        assert_eq!(stripped, plain);
        assert_eq!(colored.lines().count(), 2, "one line of counts, one of times");
        assert!(!plain.contains('\x1b'));
    }
}
