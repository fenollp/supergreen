//! What each wrapped call cost, and what it saved.
//!
//! A build is many processes: cargo spawns one of us per crate, and none of them sees the
//! others. A build tells its [`BuildStats`] to the plugin (over [`crate::chitchat`]), which keeps
//! them next to the results cache: that is how replays know what they saved. A replay
//! keeps its [`ReplayStats`] in the crate's `Md`, in the target dir. The plugin — the one process
//! that outlives the build — then reads back the `Md`s written since it started cargo, and sums
//! it all up into the report at [`Report::report`].
//!
//! Every number here is measured, never guessed.

use std::{
    borrow::Cow,
    collections::HashMap,
    ffi::OsStr,
    fmt::Write as _,
    time::{Instant, SystemTime},
};

use camino::{Utf8Path, Utf8PathBuf};
use log::debug;
use serde::{Deserialize, Serialize};

use crate::{
    PKG,
    md::{Md, MdId},
    redact::Vars,
    sys::fs,
};

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

/// One crate's worth, as read back from its `Md`.
#[derive(Debug, Clone)]
struct Crate {
    this: MdId,
    /// All of them, transitive ones included
    deps: Vec<MdId>,
    built: Option<BuildStats>,
    /// Set when this run replayed `built` rather than building it.
    replayed: Option<ReplayStats>,
}

/// The whole run, as read back from the `Md`s it wrote.
#[derive(Debug, Default)]
pub(crate) struct Report {
    crates: Vec<Crate>,
    /// How long the build took, as the one who waited on it saw it.
    wall_ms: u64,
}

impl Report {
    /// Reads back the stats of the `Md`s written since `since`, under `target_dir`, given what
    /// wrappers told of their `built` crates.
    ///
    /// `Md`s sit in profile dirs: `$CARGO_TARGET_DIR/<profile>/` and, when cross compiling,
    /// `$CARGO_TARGET_DIR/<triple>/<profile>/`. Looking two levels down finds them all without
    /// having to work out cargo's profiles and targets (`--release`, `--profile`, `--target`s,
    /// `build.target`, ...). Older `Md`s (Fresh crates, earlier runs) are older than `since`.
    #[must_use]
    pub(crate) fn read(
        target_dir: &Utf8Path,
        since: SystemTime,
        wall_ms: u64,
        built: &HashMap<MdId, BuildStats>,
    ) -> Self {
        let subdirs = |dir: &Utf8Path| -> Vec<Utf8PathBuf> {
            let entries = fs().read_dir(dir).unwrap_or_default();
            entries.into_iter().filter(|path| fs().is_dir(path)).collect()
        };
        let mut dirs = subdirs(target_dir);
        dirs.extend(dirs.clone().iter().flat_map(|dir| subdirs(dir)));

        let mut crates = vec![];
        for dir in dirs {
            for path in fs().read_dir(&dir).unwrap_or_default() {
                if !is_md_file(&path) || fs().modified(&path).is_ok_and(|t| t < since) {
                    continue;
                }
                let Ok(txt) = fs().read_to_string(&path) else { continue };
                match txt.parse::<Md>() {
                    Ok(md) => {
                        let (this, replayed) = (md.this(), md.replayed);
                        let built = built.get(&this).copied();
                        if built.is_some() || replayed.is_some() {
                            crates.push(Crate { this, deps: md.deps().collect(), built, replayed });
                        }
                    }
                    Err(e) => debug!("skipping unreadable Md {path}: {e}"),
                }
            }
        }
        Self { crates, wall_ms }
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.crates.is_empty()
    }

    /// Crates this run built.
    fn builds(&self) -> impl Iterator<Item = BuildStats> + '_ {
        self.crates.iter().filter(|c| c.replayed.is_none()).filter_map(|c| c.built)
    }

    /// Crates this run replayed, with what building them had cost (when that was recorded).
    fn replays(&self) -> impl Iterator<Item = (ReplayStats, BuildStats)> + '_ {
        self.crates.iter().filter_map(|c| Some((c.replayed?, c.built.unwrap_or_default())))
    }

    /// How long building all of this run's crates takes at the very least, however parallel the
    /// build: its longest chain of builds, deps first (each crate's build waits on its deps' as
    /// we hand cargo no early `.rmeta`). Replayed crates count for what building them took.
    /// Deps without an `Md` from this run (Fresh crates) count for nothing.
    fn critical_path_ms(&self) -> u64 {
        fn longest(
            this: MdId,
            crates: &HashMap<MdId, &Crate>,
            memo: &mut HashMap<MdId, u64>,
        ) -> u64 {
            if let Some(&ms) = memo.get(&this) {
                return ms;
            }
            let Some(krate) = crates.get(&this) else { return 0 };
            let deps = krate.deps.iter().map(|&dep| longest(dep, crates, memo)).max();
            let ms =
                deps.unwrap_or_default() + krate.built.map(|b| b.building_ms).unwrap_or_default();
            let _ = memo.insert(this, ms);
            ms
        }
        let crates = self.crates.iter().map(|krate| (krate.this, krate)).collect();
        let mut memo = HashMap::new();
        self.crates
            .iter()
            .map(|krate| longest(krate.this, &crates, &mut memo))
            .max()
            .unwrap_or_default()
    }

    /// The report, as printed once the build is over, maybe in colors.
    #[must_use]
    pub(crate) fn report(&self, color: bool) -> String {
        let paint = |style: &str, txt: &str| {
            if color { format!("\x1b[{style}m{txt}\x1b[0m") } else { txt.to_owned() }
        };
        let bold = |txt: &str| paint("1", txt);
        // Cargo's own status style: bold bright green labels, right-aligned on 12 columns.
        let label = |txt: &str| paint("1;92", &format!("{txt:>12}"));

        let (replayed, built) = (self.replays().count(), self.builds().count());
        let remotely = self.builds().filter(|b| b.remote).count();

        let mut counts = vec![];
        for (n, what) in [(replayed, "replayed"), (built, "built"), (remotely, "remotely")] {
            if n != 0 {
                let n = n.to_string();
                let n = if what == "replayed" { bold(&n) } else { n };
                counts.push(format!("{n} {what}"));
            }
        }

        let mut lines = String::new();
        let _ = writeln!(
            lines,
            "{} {} crate{} — {}",
            paint("1;92", &format!("{PKG}:")),
            self.crates.len(),
            if self.crates.len() == 1 { "" } else { "s" },
            counts.join(", "),
        );

        // Everything but the wall time is added up over crates that mostly ran side by side.
        let replaying: u64 = self.replays().map(|(r, _)| r.replaying_ms).sum();
        let mut times = vec![];
        for (what, ms) in [
            ("wall", self.wall_ms),
            ("replaying", replaying),
            ("building", self.builds().map(|b| b.building_ms).sum()),
        ] {
            if ms != 0 {
                let ms = secs(ms);
                let ms = if what == "wall" { bold(&ms) } else { ms };
                times.push(format!("{what} {ms}"));
            }
        }
        if !times.is_empty() {
            let _ = writeln!(lines, "{}  {}", label("time"), times.join(" · "));
        }

        // Building it all could not have been quicker than its critical path: of wall time, we
        // saved at least that minus this run's. In total: what the replayed builds took, minus
        // what replaying them took.
        let mut saved = vec![];
        let wall = self.critical_path_ms().saturating_sub(self.wall_ms);
        if wall != 0 && replayed != 0 {
            saved.push(format!("at least {} of wall time", bold(&secs(wall))));
        }
        let was: u64 = self.replays().map(|(_, b)| b.building_ms).sum();
        let total = was.saturating_sub(replaying);
        if total != 0 {
            saved.push(format!("{} in total", bold(&secs(total))));
        }
        if !saved.is_empty() {
            let _ = writeln!(lines, "{}  {}", label("saved"), saved.join(" · "));
        }

        // Compressed tarballs, as stored in the results cache
        let read: u64 = self.replays().map(|(r, _)| r.read_bytes).sum();
        if read != 0 {
            let _ = writeln!(lines, "{}  {} read back", label("results"), size(read));
        }

        lines
    }
}

/// `<mdid>.toml`: 16 hex digits
fn is_md_file(path: &Utf8Path) -> bool {
    path.extension() == Some("toml")
        && path
            .file_stem()
            .is_some_and(|stem| stem.len() == 16 && stem.bytes().all(|b| b.is_ascii_hexdigit()))
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
            when = args.next().map(Cow::into_owned);
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

#[cfg(test)]
mod report {
    use std::{
        collections::HashMap,
        time::{Duration, SystemTime},
    };

    use snapbox::{assert_data_eq, str};

    use super::{BuildStats, Crate, ReplayStats, Report, secs, size, wants_color};
    use crate::{
        md::MdId,
        redact::Vars,
        sys::{Sys, fake::FakeFs},
    };

    fn replayed(this: u64, deps: &[u64], building_ms: u64) -> Crate {
        Crate {
            this: this.into(),
            deps: deps.iter().copied().map(MdId::from).collect(),
            built: Some(BuildStats { building_ms, ..BuildStats::default() }),
            replayed: Some(ReplayStats { replaying_ms: 40, read_bytes: 3_000_000 }),
        }
    }

    fn built(this: u64, remote: bool, building_ms: u64) -> Crate {
        let built = BuildStats { remote, building_ms };
        Crate { this: this.into(), deps: vec![], built: Some(built), replayed: None }
    }

    #[test]
    fn a_build_that_replayed_everything_says_what_it_saved() {
        let report = Report {
            crates: vec![
                replayed(1, &[], 4_000),
                replayed(2, &[1], 32_000), // Waits on 1: a 36s critical path
                replayed(3, &[], 20_000),  // Built alongside them
            ],
            wall_ms: 1_200,
        };

        assert_data_eq!(
            report.report(false),
            str![[r#"
cargo-green: 3 crates — 3 replayed
        time  wall 1.20s · replaying 120ms
       saved  at least 34.80s of wall time · 55.88s in total
     results  8.6MiB read back

"#]]
        );
    }

    #[test]
    fn a_build_says_what_it_built() {
        let report = Report {
            crates: vec![built(1, true, 92_000), built(2, false, 8_000)],
            wall_ms: 94_000,
        };

        assert_data_eq!(
            report.report(false),
            str![[r#"
cargo-green: 2 crates — 2 built, 1 remotely
        time  wall 1m34s · building 1m40s

"#]]
        );
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

    /// The plugin finds this run's `Md`s, wherever cargo's profiles and targets put them.
    #[test]
    fn stats_are_read_back_from_this_run_s_mds() {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        let target_dir = "/home/u/wrk/target";

        let md = |this: &str, stats: &str| {
            format!("stamp = 3\nthis = \"{this}\"\nstages = []\n\n{stats}")
        };
        let stats = |building_ms| BuildStats { building_ms, ..BuildStats::default() };
        let built = HashMap::from([
            (MdId::from("0000000000000001"), stats(4_000)),
            (MdId::from("0000000000000002"), stats(9_000)),
            (MdId::from("0000000000000003"), stats(1)),
        ]);
        let started = SystemTime::now();
        fs.file(format!("{target_dir}/debug/0000000000000001.toml"), md("0000000000000001", ""));
        fs.file(
            format!("{target_dir}/x86_64-unknown-linux-gnu/release/0000000000000002.toml"),
            md("0000000000000002", "[replayed]\nreplaying_ms = 30"),
        );
        // An earlier run's (or a Fresh crate's)
        let stale = format!("{target_dir}/debug/0000000000000003.toml");
        fs.file(&stale, md("0000000000000003", ""));
        fs.modified_at(&stale, started - Duration::from_secs(60));
        // Not Mds, or without stats (e.g. a failed build)
        fs.file(format!("{target_dir}/debug/Cargo.toml"), "[package]");
        fs.file(
            format!("{target_dir}/debug/0000000000000004.toml"),
            "stamp = 3\nthis = \"0000000000000004\"\nstages = []\n",
        );

        let report = Report::read(target_dir.into(), started, 1_200, &built);
        assert_eq!(report.crates.len(), 2);
        assert_eq!(report.builds().map(|b| b.building_ms).collect::<Vec<_>>(), [4_000]);
        let replays: Vec<_> =
            report.replays().map(|(r, b)| (r.replaying_ms, b.building_ms)).collect();
        assert_eq!(replays, [(30, 9_000)]);
    }

    #[test]
    fn colors_are_cargo_s_status_colors() {
        let report = Report { crates: vec![replayed(1, &[], 4_000)], wall_ms: 1_200 };
        let colored = report.report(true);
        assert!(
            colored
                .starts_with("\x1b[1;92mcargo-green:\x1b[0m 1 crate — \x1b[1m1\x1b[0m replayed\n"),
            "{colored:?}"
        );
        assert!(colored.contains("  wall \x1b[1m1.20s\x1b[0m"), "{colored:?}");
        assert!(
            colored.contains(
                "\x1b[1;92m       saved\x1b[0m  at least \x1b[1m2.80s\x1b[0m of wall time · \x1b[1m3.96s\x1b[0m in total"
            ),
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
