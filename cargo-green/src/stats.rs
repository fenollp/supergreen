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

use crate::{PKG, sys::sys};

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

/// Why a recipe found no result to replay, in the words of the recipe itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Miss {
    /// Short reason, e.g. `env CARGO_RUSTC_CURRENT_DIR` or `base image`.
    pub(crate) why: String,
    /// The lines that differ, `-` for the result we had, `+` for the one we want.
    pub(crate) diff: Vec<String>,
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

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) miss: Option<Miss>,
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
        if let Err(e) = sys().fs.append(Utf8Path::new(&path), &format!("{line}\n")) {
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
        let stats = match sys().fs.read_to_string(path) {
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
        let _ = sys().fs.remove_file(path);
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

        let misses: Vec<_> =
            self.stats.iter().filter_map(|s| Some((&s.krate, s.miss.as_ref()?))).collect();
        if !misses.is_empty() {
            let mut whys: Vec<(&str, usize)> = vec![];
            for (_, miss) in &misses {
                match whys.iter_mut().find(|(why, _)| *why == miss.why) {
                    Some((_, count)) => *count += 1,
                    None => whys.push((&miss.why, 1)),
                }
            }
            whys.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            let whys: Vec<_> = whys.iter().map(|(why, count)| format!("{why} ({count})")).collect();
            let _ = writeln!(lines, "     missed  {} · {}", misses.len(), whys.join(" · "));

            for (krate, miss) in misses.iter().take(MISSES_SHOWN) {
                let _ = writeln!(lines, "             {krate}: {}", miss.why);
                for line in &miss.diff {
                    let _ = writeln!(lines, "               {line}");
                }
            }
            if misses.len() > MISSES_SHOWN {
                let _ = writeln!(lines, "             … and {} more", misses.len() - MISSES_SHOWN);
            }
        }

        // Sources and images are fetched by the runner, inside its own build: those bytes and
        // that wait never cross this process. Neither does a networked cache's, for now.
        let _ = writeln!(lines, "    network  unmeasured: the runner does its own fetching");

        lines
    }
}

const MISSES_SHOWN: usize = 5;

/// What the results cache takes up, and how many results that is.
///
/// Reads sizes off the real filesystem: only the plugin ever calls this, once, after the
/// build, and the tests below hand [`Report`] its numbers rather than a directory.
#[must_use]
fn du(dir: &Utf8Path) -> Option<(u64, usize)> {
    let mut bytes = 0;
    let mut count = 0;
    for entry in sys().fs.read_dir(dir).ok()? {
        // Each result is a tarball, plus the small file saying what it came out of.
        count += usize::from(entry.ends_with(".tar.gz"));
        if let Ok(md) = std::fs::metadata(dir.join(entry)) {
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

/// Names the difference between the recipe we want and the closest one we have.
///
/// Recipes are written line by line and in a stable order, so the first line that differs is
/// the change that cost us the result: what it *is* is readable off the line itself.
#[must_use]
pub(crate) fn why_missed(had: &str, wants: &str) -> Miss {
    let (mut had, mut wants) = (had.lines(), wants.lines());
    loop {
        let (a, b) = (had.next(), wants.next());
        if a == b {
            if a.is_none() {
                // Same recipe (or both ran out): nothing to explain, which is a caller's bug.
                return Miss { why: "recipe".to_owned(), diff: vec![] };
            }
            continue;
        }
        // `rustc` calls run long: show the neighbourhood of the change, not its line number.
        let at = a
            .unwrap_or_default()
            .chars()
            .zip(b.unwrap_or_default().chars())
            .take_while(|(x, y)| x == y)
            .count();
        let diff =
            [a.map(|l| format!("- {}", around(l, at))), b.map(|l| format!("+ {}", around(l, at)))]
                .into_iter()
                .flatten()
                .collect();
        return Miss { why: name_of(a.unwrap_or_default(), b.unwrap_or_default()), diff };
    }
}

/// How much of a recipe line a report shows.
const WIDTH: usize = 110;

/// The stretch of a line around the character `at`, elided on either side as needed.
#[must_use]
fn around(line: &str, at: usize) -> String {
    let line = line.trim_end();
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= WIDTH {
        return line.to_owned();
    }
    let start = at.saturating_sub(WIDTH / 3).min(chars.len().saturating_sub(WIDTH));
    let end = (start + WIDTH).min(chars.len());
    let elided = |yes| if yes { "…" } else { "" };
    format!(
        "{}{}{}",
        elided(start != 0),
        chars[start..end].iter().collect::<String>().trim(),
        elided(end != chars.len())
    )
}

/// Reads back what a recipe line is about, so a miss reads as a cause and not as a diff.
#[must_use]
fn name_of(had: &str, wants: &str) -> String {
    for line in [wants, had] {
        let trimmed = line.trim_start().trim_end_matches(&[' ', '\\'][..]);
        if line.starts_with("FROM ") {
            return "base image".to_owned();
        }
        if trimmed.contains("apt-get ") || trimmed.contains("apk add") {
            return "system packages".to_owned();
        }
        if trimmed.contains("rustup component") {
            return "toolchain components".to_owned();
        }
        if trimmed.contains("rustup ") || trimmed.contains("RUSTUP_TOOLCHAIN=") {
            return "toolchain".to_owned();
        }
        if let Some(assignment) = trimmed.strip_prefix("env ").unwrap_or(trimmed).split(' ').next()
            && let Some((var, _)) = assignment.split_once('=')
            && var.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            && !var.is_empty()
        {
            return format!("env {var}");
        }
        if trimmed.starts_with("rustc ") {
            return "rustc call".to_owned();
        }
        if trimmed.starts_with("--mount=") {
            return "mounted dependency".to_owned();
        }
        if trimmed.starts_with("ADD ") || trimmed.starts_with("COPY ") {
            return "sources".to_owned();
        }
    }
    "recipe".to_owned()
}

/// Where the plugin has this run's tally written.
#[must_use]
pub(crate) fn path_for(tmp: &Utf8Path, run: &str) -> Utf8PathBuf {
    tmp.join(format!("{PKG}-{run}.stats.jsonl"))
}

#[cfg(test)]
mod tally {
    use snapbox::{assert_data_eq, str};

    use super::{Miss, Outcome, Report, Stat, size, why_missed};

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
    fn a_miss_is_reported_as_what_changed() {
        let report = Report {
            stats: vec![Stat {
                outcome: Some(Outcome::Built),
                keying_ms: 4,
                building_ms: 92_000,
                stored_bytes: 12_000_000,
                wrote_bytes: 30_000_000,
                miss: Some(Miss {
                    why: "env CARGO_RUSTC_CURRENT_DIR".to_owned(),
                    diff: vec!["- env CARGO_PKG_NAME=serde".to_owned()],
                }),
                ..Stat::of("serde v1.0.228")
            }],
            cache: None,
            wall_ms: 94_000,
        };

        assert_data_eq!(
            report.report(),
            str![[r#"
cargo-green: 1 crate — 0 replayed, 1 built, 0 compiled locally
       time  wall 1m34s · keys 4ms · building 1m32s (all but wall are summed over crates)
       disk  11.4MiB added · 28.6MiB written out
     missed  1 · env CARGO_RUSTC_CURRENT_DIR (1)
             serde v1.0.228: env CARGO_RUSTC_CURRENT_DIR
               - env CARGO_PKG_NAME=serde
    network  unmeasured: the runner does its own fetching

"#]]
        );
    }

    /// Every line of a recipe stands for one input of the build, so a miss can be named.
    #[test]
    fn misses_are_named_after_the_input_that_changed() {
        let recipe = "\
FROM rust:1.92 AS rust-base
FROM rust-base AS dep-serde
RUN \\
    env CARGO_PKG_NAME=serde \\
       CARGO_RUSTC_CURRENT_DIR=/work \\
  --mount=from=cratesio-serde-1.0.228,dst=/work \\
      rustc --crate-name serde src/lib.rs
";
        let named = |replaced: &str, with: &str| {
            let miss = why_missed(recipe, &recipe.replace(replaced, with));
            (miss.why, miss.diff.len())
        };

        assert_eq!(named("rust:1.92", "rust:1.93"), ("base image".to_owned(), 2));
        assert_eq!(
            named("FROM rust-base AS dep-serde", "RUN apt-get install -y jq"),
            ("system packages".to_owned(), 2)
        );
        assert_eq!(
            named("CARGO_RUSTC_CURRENT_DIR=/work", "CARGO_RUSTC_CURRENT_DIR=/elsewhere"),
            ("env CARGO_RUSTC_CURRENT_DIR".to_owned(), 2)
        );
        assert_eq!(named("src/lib.rs", "src/main.rs"), ("rustc call".to_owned(), 2));
        assert_eq!(
            named("1.0.228,dst=/work", "1.0.229,dst=/work"),
            ("mounted dependency".to_owned(), 2)
        );

        // A recipe that grew a line still names the input it grew.
        let miss = why_missed(recipe, &recipe.replace("RUN \\\n", "RUN \\\n    env FOO=1 \\\n"));
        assert_eq!((miss.why.as_str(), miss.diff.len()), ("env FOO", 2));
    }

    /// A recipe line can run to a thousand characters: the change is what matters.
    #[test]
    fn long_lines_are_shown_around_what_changed() {
        let long =
            |flag: &str| format!("      rustc {}{flag} --crate-name serde", "-C x=y ".repeat(40));
        let miss = why_missed(&long("--cap-lints warn"), &long("--cap-lints allow"));
        assert_eq!(miss.why, "rustc call");
        assert_data_eq!(
            miss.diff.join("\n"),
            str![[r#"
- …x=y -C x=y -C x=y -C x=y -C x=y -C x=y -C x=y -C x=y -C x=y -C x=y -C x=y --cap-lints warn --crate-name serde
+ …x=y -C x=y -C x=y -C x=y -C x=y -C x=y -C x=y -C x=y -C x=y -C x=y -C x=y --cap-lints allow --crate-name serde
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
