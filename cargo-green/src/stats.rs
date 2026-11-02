//! What each wrapped call cost, and what it saved.
//!
//! A build is many processes: cargo spawns one of us per crate, and none of them sees the
//! others. So each writes a file about its own crate in a directory of the target
//! dir, and the plugin — the one process that outlives the build — sums those up into the
//! report at [`Report::report`].
//!
//! Every number here is measured, never guessed: what we cannot time from inside these
//! processes (the runner's own fetching, and one day a networked cache's transfers) is
//! reported as unmeasured rather than as zero.

use std::{
    ffi::OsStr,
    fmt::Write as _,
    time::{Duration, Instant},
};

use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, warn};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{PKG, redact::Vars, sys::fs};

/// What became of one wrapped `rustc` (or `build.rs`) call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Outcome {
    /// A past build of this very stage was unpacked instead of being run again.
    Replayed,
    /// The runner built it (see [`Stat::remote`] for where).
    Built,
    /// Handed back to cargo's own `rustc`, as if we weren't there: no runner and no result.
    Unwrapped,
}

/// One crate's worth of accounting.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Stat {
    pub(crate) krate: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) outcome: Option<Outcome>,

    /// Built on a remote daemon (`ssh://` or `tcp://`, reached through our connection pool).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) remote: bool,

    /// Unpacking a replayed result over the target dir.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) replaying_ms: u64,

    /// The runner, from handing it the recipe to having its artifacts on disk.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) building_ms: u64,

    /// Cargo's own `rustc`, when the call was handed back to it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) unwrapped_ms: u64,

    /// Compressing then writing the result tarball, once the artifacts are in the target dir.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) storing_ms: u64,

    /// What the build we replayed had itself taken, when it recorded that.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) saved_ms: u64,

    /// Result tarball read back (compressed).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) read_bytes: u64,

    /// Result tarball kept for later (compressed).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) stored_bytes: u64,

    /// Artifacts landed in the target dir.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) wrote_bytes: u64,
}

impl Stat {
    #[must_use]
    pub(crate) fn of(krate: &str) -> Self {
        Self { krate: krate.to_owned(), ..Default::default() }
    }

    /// Adds this to the run's tally, if the plugin set one up.
    ///
    /// Never fails a build: a report is worth less than the artifacts it describes.
    pub(crate) fn record(&self, target_dir: Option<&Utf8Path>) {
        let Some(target_dir) = target_dir else { return };
        let dir = tally_dir(target_dir);
        if !fs().is_dir(&dir) {
            return; // Not wrapped by the plugin: no one to read this.
        }
        let data = match serde_json::to_string(self) {
            Ok(data) => data,
            Err(e) => return warn!("Failed serializing stats for {}: {e}", self.krate),
        };
        // One file per call, named so that no two of cargo's concurrent calls ever share one;
        // the plugin reads them only once cargo is done.
        let path = dir.join(format!("{}.json", Uuid::new_v4()));
        if let Err(e) = fs().write(&path, &data) {
            warn!("Failed writing stats to {path}: {e}");
        }
    }
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

/// The whole run, as read back from the tally.
#[derive(Debug, Default)]
pub(crate) struct Report {
    stats: Vec<Stat>,
    /// How long the build took, as the one who waited on it saw it.
    wall_ms: u64,
}

impl Report {
    /// Reads the tally the wrapper processes left behind.
    #[must_use]
    pub(crate) fn read(dir: &Utf8Path, wall_ms: u64) -> Self {
        let entries = match fs().read_dir(dir) {
            Err(e) => {
                debug!("no stats in {dir}: {e}");
                return Self::default();
            }
            Ok(entries) => entries,
        };
        let stats = entries
            .into_iter()
            .filter_map(|entry| Some(dir.join(entry.file_name()?)))
            .filter(|path| path.extension() == Some("json"))
            .filter_map(|path| {
                let data = fs().read_to_string(&path).ok()?;
                serde_json::from_str(&data)
                    .inspect_err(|e| warn!("Dropping corrupted stats {path}: {e}"))
                    .ok()
            })
            .collect();
        Self { stats, wall_ms }
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

    /// The report, as printed once the build is over, maybe in colors.
    #[must_use]
    pub(crate) fn report(&self, color: bool) -> String {
        let paint = |style: &str, txt: &str| {
            if color { format!("\x1b[{style}m{txt}\x1b[0m") } else { txt.to_owned() }
        };
        // Cargo's own status style: bold bright green labels, right-aligned on 12 columns.
        let label = |txt: &str| paint("1;92", &format!("{txt:>12}"));

        let (replayed, built, unwrapped) = (
            self.count(Outcome::Replayed),
            self.count(Outcome::Built),
            self.count(Outcome::Unwrapped),
        );
        let remotely = self.stats.iter().filter(|s| s.remote).count();

        let mut lines = String::new();
        let _ = writeln!(
            lines,
            "{} {} crate{} — {replayed} replayed, {built} built, {remotely} remotely, {unwrapped} unwrapped",
            paint("1;92", &format!("{PKG}:")),
            self.stats.len(),
            if self.stats.len() == 1 { "" } else { "s" },
        );

        // Everything but the wall time is added up over crates that mostly ran side by side.
        let mut times = vec![];
        for (what, ms) in [
            ("wall", self.wall_ms),
            ("replaying", self.sum(|s| s.replaying_ms)),
            ("building", self.sum(|s| s.building_ms)),
            ("storing", self.sum(|s| s.storing_ms)),
            ("unwrapped", self.sum(|s| s.unwrapped_ms)),
        ] {
            if ms != 0 {
                times.push(format!("{what} {}", secs(ms)));
            }
        }
        if !times.is_empty() {
            let _ = writeln!(lines, "{}  {}", label("time"), times.join(" · "));
        }

        let saved = self.sum(|s| s.saved_ms);
        if saved != 0 {
            let _ = writeln!(
                lines,
                "{}  {} of building, for {} of unpacking",
                label("saved"),
                paint("1", &secs(saved)),
                secs(self.sum(|s| s.replaying_ms))
            );
        }

        // The results cache (compressed tarballs) and the target dir (cargo's artifacts) are
        // different places holding different bytes: never added up.
        let mut results = vec![];
        let stored = self.sum(|s| s.stored_bytes);
        if stored != 0 {
            results.push(format!("{} added", size(stored)));
        }
        let read = self.sum(|s| s.read_bytes);
        if read != 0 {
            results.push(format!("{} read back", size(read)));
        }
        if !results.is_empty() {
            let _ = writeln!(lines, "{}  {}", label("results"), results.join(" · "));
        }
        let wrote = self.sum(|s| s.wrote_bytes);
        if wrote != 0 {
            let _ = writeln!(lines, "{}  {} written", label("target"), size(wrote));
        }

        // Sources and images are fetched by the runner, inside its own build: those bytes and
        // that wait never cross this process. Neither does a networked cache's, for now.
        let _ =
            writeln!(lines, "{}  unmeasured: the runner does its own fetching", label("network"));

        lines
    }
}

/// Whether to color the report, the way cargo decides for its own output:
/// `--color <when>`, else `$CARGO_TERM_COLOR`, else when stderr is a terminal and `$NO_COLOR` is unset.
#[must_use]
pub(crate) fn wants_color<'a>(
    cargo_args: impl IntoIterator<Item = &'a OsStr>,
    env: &Vars,
    stderr_is_tty: bool,
) -> bool {
    let mut when = None;
    let mut args = cargo_args.into_iter().map(OsStr::to_string_lossy);
    while let Some(arg) = args.next() {
        if arg == "--" {
            break; // What follows is for the program cargo runs
        }
        if arg == "--color" {
            when = args.next().map(|when| when.into_owned());
        } else if let Some(val) = arg.strip_prefix("--color=") {
            when = Some(val.to_owned());
        }
    }
    let when = when.or_else(|| env.get_nonempty("CARGO_TERM_COLOR").map(ToOwned::to_owned));
    match when.as_deref() {
        Some("always") => true,
        Some("never") => false,
        _ => stderr_is_tty && env.get_nonempty("NO_COLOR").is_none(),
    }
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

/// Where wrapped calls drop their stats, one file each.
///
/// Under the target dir: both plugin and wrappers know it, and cargo holds its lock for the
/// whole build so no two runs share it at once.
#[must_use]
pub(crate) fn tally_dir(target_dir: &Utf8Path) -> Utf8PathBuf {
    target_dir.join(format!("{PKG}-stats"))
}

#[cfg(test)]
mod tally {
    use camino::Utf8Path;
    use snapbox::{assert_data_eq, str};

    use super::{Outcome, Report, Stat, size, tally_dir, wants_color};
    use crate::{
        redact::Vars,
        sys::{Sys, fake::FakeFs},
    };

    fn replayed(krate: &str, saved_ms: u64) -> Stat {
        Stat {
            outcome: Some(Outcome::Replayed),
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
            wall_ms: 1_200,
        };

        assert_data_eq!(
            report.report(false),
            str![[r#"
cargo-green: 2 crates — 2 replayed, 0 built, 0 remotely, 0 unwrapped
        time  wall 1.2s · replaying 80ms
       saved  36.0s of building, for 80ms of unpacking
     results  5.7MiB read back
      target  17.2MiB written
     network  unmeasured: the runner does its own fetching

"#]]
        );
    }

    #[test]
    fn a_build_says_what_it_built_and_kept() {
        let report = Report {
            stats: vec![
                Stat {
                    outcome: Some(Outcome::Built),
                    remote: true,
                    building_ms: 92_000,
                    storing_ms: 300,
                    stored_bytes: 12_000_000,
                    wrote_bytes: 30_000_000,
                    ..Stat::of("serde v1.0.228")
                },
                Stat { outcome: Some(Outcome::Built), building_ms: 8_000, ..Stat::of("cc v1.2.0") },
                Stat {
                    outcome: Some(Outcome::Unwrapped),
                    unwrapped_ms: 1_500,
                    ..Stat::of("mycrate v0.1.0")
                },
            ],
            wall_ms: 94_000,
        };

        assert_data_eq!(
            report.report(false),
            str![[r#"
cargo-green: 3 crates — 0 replayed, 2 built, 1 remotely, 1 unwrapped
        time  wall 1m34s · building 1m40s · storing 300ms · unwrapped 1.5s
     results  11.4MiB added
      target  28.6MiB written
     network  unmeasured: the runner does its own fetching

"#]]
        );
    }

    #[test]
    fn stats_roundtrip_as_tally_lines() {
        let stat = replayed("anyhow v1.0.100", 4_000);
        let line = serde_json::to_string(&stat).unwrap();
        assert!(!line.contains('\n'), "{line}");
        let back: Stat = serde_json::from_str(&line).unwrap();
        assert_eq!(back.krate, stat.krate);
        assert_eq!(back.outcome, Some(Outcome::Replayed));
        assert_eq!(back.saved_ms, 4_000);

        let sparse: Stat = serde_json::from_str(r#"{"krate":"x v1","outcome":"built"}"#).unwrap();
        assert_eq!(sparse.outcome, Some(Outcome::Built));
        assert_eq!(sparse.building_ms, 0);
    }

    /// Concurrent wrappers each drop a whole file; the plugin finds them all.
    #[test]
    fn wrapped_calls_tally_into_the_target_dir() {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        let target_dir = Utf8Path::new("/home/u/wrk/target");

        // Without the plugin having set up a tally, recording is a no-op.
        replayed("anyhow v1.0.100", 4_000).record(Some(target_dir));
        replayed("anyhow v1.0.100", 4_000).record(None);
        assert!(fs.written().is_empty(), "{:?}", fs.written());

        let dir = tally_dir(target_dir);
        fs.mkdir(&dir);
        replayed("anyhow v1.0.100", 4_000).record(Some(target_dir));
        replayed("serde v1.0.228", 32_000).record(Some(target_dir));
        assert_eq!(fs.written().len(), 2);

        let report = Report::read(&dir, 1_200);
        let mut krates: Vec<_> = report.stats.iter().map(|s| s.krate.as_str()).collect();
        krates.sort_unstable();
        assert_eq!(krates, ["anyhow v1.0.100", "serde v1.0.228"]);
        assert_eq!(report.sum(|s| s.saved_ms), 36_000);
    }

    #[test]
    fn colors_are_cargo_s_status_colors() {
        let report = Report { stats: vec![replayed("anyhow v1.0.100", 4_000)], wall_ms: 1_200 };
        let colored = report.report(true);
        assert!(colored.starts_with("\x1b[1;92mcargo-green:\x1b[0m 1 crate"), "{colored:?}");
        assert!(
            colored.contains("\x1b[1;92m       saved\x1b[0m  \x1b[1m4.0s\x1b[0m"),
            "{colored:?}"
        );
        assert!(!report.report(false).contains('\x1b'));
    }

    #[test]
    fn colors_are_decided_as_cargo_does() {
        use std::ffi::OsStr;
        let args = |args: &'static [&'static str]| args.iter().map(OsStr::new);
        let none = Vars::from([]);

        assert!(wants_color(args(&["build"]), &none, true), "auto on a terminal");
        assert!(!wants_color(args(&["build"]), &none, false), "auto off a terminal");
        assert!(!wants_color(args(&["build"]), &Vars::from([("NO_COLOR", "1")]), true));
        assert!(wants_color(args(&["build"]), &Vars::from([("NO_COLOR", "")]), true));

        let always = Vars::from([("CARGO_TERM_COLOR", "always")]);
        let never = Vars::from([("CARGO_TERM_COLOR", "never")]);
        assert!(wants_color(args(&["build"]), &always, false));
        assert!(!wants_color(args(&["build"]), &never, true));
        assert!(wants_color(args(&["build"]), &Vars::from([("CARGO_TERM_COLOR", "auto")]), true));

        // The flag wins over the environment, and over $NO_COLOR
        assert!(wants_color(args(&["build", "--color=always"]), &never, false));
        assert!(!wants_color(args(&["build", "--color", "never"]), &always, true));
        let no_color = Vars::from([("NO_COLOR", "1")]);
        assert!(wants_color(args(&["build", "--color", "always"]), &no_color, false));

        // Flags after `--` are the program's, not cargo's
        assert!(!wants_color(args(&["run", "--", "--color=always"]), &none, false));
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
