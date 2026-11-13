//! `cargo green supergreen doctor`: reports on the host (symlinks, runners, XDG dirs, what we keep
//! there) then checks properties of BuildKit (and of its Dockerfile frontend) that we rely on
//! against the builder and frontend actually in use, called with the flags our own builds use. So
//! that upgrading either can't silently break what results' and the build cache's reproducibility
//! (or wrappers' reuse of their deps' stages) stands on.
//!
//! Builds on the configured builder and pulls `IMAGE`, unless `$CARGOGREEN_RUNNER=none`.

use anyhow::{Error, Result, anyhow, bail};
use camino::Utf8Path;

use self::{
    buildkit::nonce,
    cargo_home_is_followable::{CARGO_HOME_LINK, cargo_home_is_followable},
    context_files_mtimes_are_neither_cache_keys_nor_exported::{
        CONTEXT_MTIMES, context_files_mtimes_are_neither_cache_keys_nor_exported,
    },
    copy_reuses_its_cached_output_whatever_its_sources_mtimes::{
        COPY_REUSE, copy_reuses_its_cached_output_whatever_its_sources_mtimes,
    },
    cratesio_index_is_linked::{CRATESIO_INDEX_LINK, cratesio_index_is_linked},
    dir_is_usable::{XDG_BUILDKIT, XDG_RESULTS, XDG_VAR_RUN, dir_is_usable},
    host::{XDG_CACHE_RUN, XDG_RUNTIME, cache_sizes, envs_in_use, runners, sockets_in},
    mount_stubs_leave_their_parent_dirs_as_the_run_did::{
        MOUNT_STUBS, mount_stubs_leave_their_parent_dirs_as_the_run_did,
    },
    tar_exporter_rewrites_produced_files_timestamps::{
        TAR_EXPORTER_REWRITES, tar_exporter_rewrites_produced_files_timestamps,
    },
    tar_exporter_writes_deterministic_tarballs::{
        DETERMINISTIC_TARBALLS, tar_exporter_writes_deterministic_tarballs,
    },
    workdir_timestamps_only_its_leaf_dir::{WORKDIR_LEAF, workdir_timestamps_only_its_leaf_dir},
};
use crate::{
    REPO,
    all_our_envs::{BUILDX_BUILDER, CARGOGREEN_BUILDER_IMAGE, CARGOGREEN_SYNTAX_IMAGE},
    cache::Cache,
    coloring::Paint,
    dirs::{Dirs, Paths},
    green::Green,
    sys::{VAR_RUN_MODE, fs},
};

mod buildkit;
mod cargo_home_is_followable;
mod context_files_mtimes_are_neither_cache_keys_nor_exported;
mod copy_reuses_its_cached_output_whatever_its_sources_mtimes;
mod cratesio_index_is_linked;
mod dir_is_usable;
mod host;
mod mount_stubs_leave_their_parent_dirs_as_the_run_did;
mod tar_exporter_rewrites_produced_files_timestamps;
mod tar_exporter_writes_deterministic_tarballs;
mod workdir_timestamps_only_its_leaf_dir;

struct Check {
    /// Whether it's about the builder or its frontend
    builder: bool,
    name: &'static str,
    expected: &'static str,
    why: &'static str,
    fixes: &'static [&'static str],
}

pub(crate) const SETUP: &str =
    "{ cargo green supergreen setup 2>/dev/null || true; } | sudo /bin/sh -xe";

const CHOWN: &str = r#"Give it back to yourself: `sudo chown -R "$USER" DIR`, or remove it: all of it can be remade"#;

const RUNNER_FOUND: Check = Check {
    name: "runner in use is installed",
    expected: concat!("$", CARGOGREEN_RUNNER!(), " resolves to an executable through $", PATH!()),
    why: "Every crate gets compiled in a container, started by this program. Without it, nothing can be built.",
    fixes: &[concat!(
        "Install it, or pick one of the runners listed above with $",
        CARGOGREEN_RUNNER!()
    )],
    builder: false,
};

const BUILDER_READY: Check = Check {
    name: "builder in use is ready",
    expected: "the builder is up and images resolve, as they must before any build",
    why: "The builder is the BuildKit instance that runs every compilation. If it's down or the images it needs can't be found, every build fails the same way, and the checks below can't run.",
    fixes: &["Is the daemon up and reachable? `docker info`, `docker buildx inspect`"],
    builder: true,
};

/// What came of a check.
enum Outcome {
    Held,
    Failed(Error),
    Skipped(&'static str),
}

impl From<Result<()>> for Outcome {
    fn from(res: Result<()>) -> Self {
        res.map_or_else(Self::Failed, |()| Self::Held)
    }
}

impl Outcome {
    #[must_use]
    fn failed(&self) -> bool {
        matches!(self, Self::Failed(_))
    }
}

const NO_RUNNER: &str = concat!("$", CARGOGREEN_RUNNER!(), "=none");

const NOT_BITREPRO: &str = concat!("not ", CARGOGREEN_EXPERIMENT!(), "=bitrepro");

const NO_BUILDER: &str = "the builder isn't ready";

/// Whether we can make a file in `dir`: tried for real, then cleaned up
fn can_write_in(dir: &Utf8Path) -> Result<()> {
    let probe = dir.join(format!(".cargo-green-doctor-{}", nonce()?));
    fs().write(&probe, "").map_err(|e| anyhow!("Can't write in {dir}: {e}"))?;
    fs().remove_file(&probe).map_err(|e| anyhow!("Can't remove from {dir}: {e}"))
}

#[must_use]
fn what_is_at(path: &Utf8Path) -> String {
    if fs().is_symlink(path) {
        return match fs().read_link(path) {
            Ok(target) => format!("{path} links to {target}"),
            Err(e) => format!("{path} is a link: {e}"),
        };
    }
    if fs().is_dir(path) {
        format!("{path} is a real directory")
    } else if fs().exists(path) {
        format!("{path} is a file")
    } else {
        format!("{path} is missing")
    }
}

/// Comes before everything else, even before reaching for the builder.
/// `cargo green` itself can't start until these hold
pub(crate) fn first_step(color: bool, paths: &Paths) -> Result<()> {
    let outcomes = [
        (&CARGO_HOME_LINK, cargo_home_is_followable(paths).into()),
        (&CRATESIO_INDEX_LINK, cratesio_index_is_linked(paths).into()),
    ];
    println!("{}", report(&Paint::for_stdout(color), &outcomes));
    verdict(&outcomes)
        .map_err(|_| anyhow!("Fix this, then run `cargo green supergreen doctor` again"))
}

/// Runs every check before telling about any: fails when one or more assumptions don't hold.
pub(crate) async fn main(mut green: Green, startup_error: Option<Error>) -> Result<()> {
    let paint = Paint::for_stdout(green.color);

    println!("{}", envs_in_use(&paint, &green));
    println!("{}", runners(&paint, green.runner).await);

    if let Some(Dirs { results, var_run, .. }) = &green.paths.dirs {
        let sizes = match results.parent().map(|root| cache_sizes(&paint, root)) {
            Some(sizes) => sizes
                .await
                .unwrap_or_else(|e| paint.section("Cache sizes", &format!("unavailable: {e:#}"))),
            None => paint.section("Cache sizes", &format!("unavailable: {results} has no parent")),
        };
        println!("{sizes}");

        // Falls back to under the cache dir when there's no $XDG_RUNTIME_DIR
        let under_cache = results.parent().is_some_and(|root| var_run.starts_with(root));
        let xdg = if under_cache { XDG_CACHE_RUN } else { XDG_RUNTIME };
        let sockets = match sockets_in(var_run) {
            Ok(names) => paint.section(
                &format!("Sockets in {var_run} ({xdg}) ({})", names.len()),
                &names.join("\n"),
            ),
            Err(e) => paint
                .section(&format!("Sockets in {var_run} ({xdg})"), &format!("unavailable: {e:#}")),
        };
        println!("{sockets}");
    }

    // Checks' first builds mustn't be found in caches, and their stages have no place in registries
    green.cache = Cache::default();

    let runner_found = if green.runner.is_none() {
        Outcome::Skipped(NO_RUNNER)
    } else {
        green.runner.executable().map(|_| ()).map_err(|e| anyhow!("{e:#}")).into()
    };
    let builder_ready = match startup_error {
        Some(e) => Outcome::Failed(e),
        None if green.runner.is_none() => Outcome::Skipped(NO_RUNNER),
        None => match green.inspect_builder().await {
            Err(e) => Outcome::Failed(e),
            Ok(inspected) => {
                println!("{}", paint.section("Builder in use", &inspected));
                Outcome::Held
            }
        },
    };
    let skip = if green.runner.is_none() {
        Some(NO_RUNNER)
    } else {
        (runner_found.failed() || builder_ready.failed()).then_some(NO_BUILDER)
    };

    // Checks take a while: each gets told as soon as it's done
    let mut outcomes = vec![];
    macro_rules! tell {
        ($check:expr, $outcome:expr) => {{
            let outcome: Outcome = $outcome;
            println!("{}", told(&paint, $check, &outcome));
            outcomes.push(($check, outcome));
        }};
    }
    macro_rules! unless_skipped {
        ($check:ident) => {
            match skip {
                Some(why) => Outcome::Skipped(why),
                None => $check(&mut green).await.into(),
            }
        };
    }

    let bitrepro = green.bitrepro();
    macro_rules! if_bitrepro {
        ($check:ident) => {
            if bitrepro { unless_skipped!($check) } else { Outcome::Skipped(NOT_BITREPRO) }
        };
    }

    let dir = |pick: fn(&Dirs) -> &Utf8Path, mode: Option<u32>| -> Outcome {
        match &green.paths.dirs {
            Some(dirs) => dir_is_usable(pick(dirs), mode).into(),
            None => Outcome::Failed(anyhow!("No valid $HOME could be retrieved from the OS")),
        }
    };

    tell!(&XDG_RESULTS, dir(|d| &d.results, None));
    tell!(&XDG_BUILDKIT, dir(|d| &d.buildkit, None));
    tell!(&XDG_VAR_RUN, dir(|d| &d.var_run, Some(VAR_RUN_MODE)));
    tell!(&RUNNER_FOUND, runner_found);
    tell!(&BUILDER_READY, builder_ready);
    tell!(&TAR_EXPORTER_REWRITES, unless_skipped!(tar_exporter_rewrites_produced_files_timestamps));
    #[rustfmt::skip]
    tell!(&CONTEXT_MTIMES, unless_skipped!(context_files_mtimes_are_neither_cache_keys_nor_exported));
    tell!(&COPY_REUSE, unless_skipped!(copy_reuses_its_cached_output_whatever_its_sources_mtimes));
    tell!(&DETERMINISTIC_TARBALLS, unless_skipped!(tar_exporter_writes_deterministic_tarballs));
    tell!(&MOUNT_STUBS, if_bitrepro!(mount_stubs_leave_their_parent_dirs_as_the_run_did));
    tell!(&WORKDIR_LEAF, if_bitrepro!(workdir_timestamps_only_its_leaf_dir));

    for block in ways_out(&paint, &outcomes) {
        println!("{block}");
    }
    verdict(&outcomes)
}

fn verdict(outcomes: &[(&Check, Outcome)]) -> Result<()> {
    let failed = outcomes.iter().filter(|(_, outcome)| outcome.failed()).count();
    let ran = outcomes.iter().filter(|(_, o)| !matches!(o, Outcome::Skipped(_))).count();
    if failed != 0 {
        bail!("{failed} of {ran} assumptions don't hold")
    }
    Ok(())
}

/// The whole report, for when there's nothing to wait on
#[must_use]
fn report(paint: &Paint, outcomes: &[(&Check, Outcome)]) -> String {
    let mut blocks: Vec<_> =
        outcomes.iter().map(|(check, outcome)| told(paint, check, outcome)).collect();
    blocks.extend(ways_out(paint, outcomes));
    blocks.join("\n")
}

/// What came of one check: details only when it failed
#[must_use]
fn told(paint: &Paint, check: &Check, outcome: &Outcome) -> String {
    let Check { name, expected, why, fixes, .. } = check;
    let details =
        |label: &str, text: &str| paint.paragraph(Paint::INDENT, &format!("*{label}:* {text}"));

    let mut block = match outcome {
        Outcome::Held => format!("  {}    {name}\n", paint.ok.apply_to("ok")),
        Outcome::Skipped(reason) => {
            format!("  {}  {name} ({reason})\n", paint.skip.apply_to("skip"))
        }
        Outcome::Failed(_) => format!("  {}  {name}\n", paint.fail.apply_to("FAIL")),
    };
    block.push('\n');
    if let Outcome::Failed(e) = outcome {
        block.push_str(&details("Expected", expected));
        block.push('\n');

        // Not ours: stays as it came, whatever it looks like
        let pad = " ".repeat(Paint::INDENT);
        block.push_str(&format!("{pad}{}\n\n", paint.label.apply_to("Got:")));
        for line in format!("{e:#}").trim().lines() {
            block.push_str(&format!("{pad}  {line}\n"));
        }
        block.push('\n');
    }
    block.push_str(&details("Why", why));
    if outcome.failed() {
        for fix in *fixes {
            block.push('\n');
            block.push_str(&details("Try", fix));
        }
    }
    block
}

/// What follows all checks' outcomes when some failed
#[must_use]
fn ways_out(paint: &Paint, outcomes: &[(&Check, Outcome)]) -> Vec<String> {
    let mut blocks = vec![];
    if outcomes.iter().any(|(check, outcome)| check.builder && outcome.failed()) {
        let mut block =
            format!("{}\n\n", paint.title.apply_to("Ways out, for what concerns the builder"));
        block.push_str(&paint.paragraph(
            2,
            &format!(
"* Build on the managed builder: unset `{BUILDX_BUILDER}`
* Upgrade the builder: set `{CARGOGREEN_BUILDER_IMAGE}` then `cargo green supergreen builder recreate`
* Upgrade the Dockerfile frontend: set `{CARGOGREEN_SYNTAX_IMAGE}`
* Start over from an empty build cache (this loses it): `cargo green supergreen builder recreate --clean`
"),
        ));
        blocks.push(block);
    }
    if outcomes.iter().any(|(_, outcome)| outcome.failed()) {
        blocks.push(paint.paragraph(
            0,
            &format!("Still failing, or unexpected? Please report it along with this output: {REPO}/issues"),
        ));
    }
    blocks
}

#[cfg(test)]
mod an_apple_a_day {
    use anyhow::anyhow;

    use crate::{
        REPO,
        coloring::Paint,
        doctor::{
            CONTEXT_MTIMES, Check, MOUNT_STUBS, NO_RUNNER, NOT_BITREPRO, Outcome,
            TAR_EXPORTER_REWRITES, WORKDIR_LEAF, XDG_RESULTS, report, verdict, what_is_at,
        },
        sys::{Fs, Sys, fake::FakeFs},
    };

    const WIDE: usize = 10_000;

    #[test]
    fn reports_list_every_failure_then_fail() {
        let outcomes = [
            (&TAR_EXPORTER_REWRITES, Outcome::Held),
            (&CONTEXT_MTIMES, Outcome::Failed(anyhow!("2 steps ran\nthe log"))),
            (&WORKDIR_LEAF, Outcome::Failed(anyhow!("leaf mtime"))),
            (&MOUNT_STUBS, Outcome::Skipped(NO_RUNNER)),
        ];

        let text = report(&Paint::plain(WIDE), &outcomes);
        assert!(text.contains(&format!("ok    {}\n", TAR_EXPORTER_REWRITES.name)), "{text}");
        assert!(text.contains(&format!("FAIL  {}\n", CONTEXT_MTIMES.name)), "{text}");
        assert!(text.contains(&format!("FAIL  {}\n", WORKDIR_LEAF.name)), "{text}");
        assert!(text.contains(&format!("skip  {} ({NO_RUNNER})\n", MOUNT_STUBS.name)), "{text}");
        assert!(text.contains(CONTEXT_MTIMES.expected), "{text}");
        assert!(text.contains("Got:\n\n          2 steps ran\n          the log\n"), "{text}");
        assert!(text.contains(WORKDIR_LEAF.fixes[0]), "{text}");
        assert!(text.contains("Ways out"), "{text}");
        assert!(text.contains(&format!("{REPO}/issues")), "{text}");

        let err = verdict(&outcomes).unwrap_err().to_string();
        assert_eq!(err, "2 of 3 assumptions don't hold");
    }

    #[test]
    fn what_only_bitrepro_relies_on_is_skipped_without_it() {
        let check @ Check { name, .. } = MOUNT_STUBS;
        let outcomes = [(&check, Outcome::Skipped(NOT_BITREPRO))];
        let text = report(&Paint::plain(WIDE), &outcomes);
        assert!(text.contains(&format!("skip  {name} (not CARGOGREEN_EXPERIMENT=bitrepro)\n")));
        assert!(verdict(&outcomes).is_ok());
    }

    #[test]
    fn host_failures_spare_the_builder_advice() {
        let outcomes = [(&XDG_RESULTS, Outcome::Failed(anyhow!("nope")))];
        let text = report(&Paint::plain(WIDE), &outcomes);
        assert!(!text.contains("Ways out"), "{text}");
        assert!(text.contains(&format!("{REPO}/issues")), "{text}");
    }

    #[test]
    fn reports_stay_short_when_none_fail() {
        let outcomes =
            [(&TAR_EXPORTER_REWRITES, Outcome::Held), (&MOUNT_STUBS, Outcome::Skipped(NO_RUNNER))];
        assert_eq!(
            report(&Paint::plain(WIDE), &outcomes),
            format!(
                "  ok    {}\n\n        Why: {}\n\n  skip  {} ({NO_RUNNER})\n\n        Why: {}\n",
                TAR_EXPORTER_REWRITES.name,
                TAR_EXPORTER_REWRITES.why,
                MOUNT_STUBS.name,
                MOUNT_STUBS.why,
            )
        );
        assert!(verdict(&outcomes).is_ok());
    }

    #[test]
    fn reports_wrap_our_text_but_not_what_we_got() {
        const WIDTH: usize = 60;
        let got = "x ".repeat(100);
        let outcomes = [(&CONTEXT_MTIMES, Outcome::Failed(anyhow!("{got}")))];

        let text = report(&Paint::plain(WIDTH), &outcomes);
        let (too_long, fit): (Vec<_>, Vec<_>) =
            text.lines().partition(|l| l.chars().count() > WIDTH);
        assert_eq!(too_long, [format!("          {}", got.trim())], "{text}");
        assert!(
            fit.iter().filter(|l| l.starts_with(&" ".repeat(Paint::INDENT))).count() > 2,
            "{text}"
        );
    }

    #[test]
    fn says_what_is_at_a_path() {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.mkdir("/d");
        fs.file("/f", "");
        fs.symlink_dir("/d".into(), "/l".into()).unwrap();

        assert_eq!(what_is_at("/g".into()), "/g is missing");
        assert_eq!(what_is_at("/d".into()), "/d is a real directory");
        assert_eq!(what_is_at("/f".into()), "/f is a file");
        assert_eq!(what_is_at("/l".into()), "/l links to /d");
    }
}
