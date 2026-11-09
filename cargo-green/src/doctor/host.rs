use std::{collections::HashSet, path::PathBuf, process::Stdio};

use anyhow::{Result, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use rustix::fs::{StatVfs, statvfs};
use tokio::{process::Command, task::spawn_blocking};

use super::NO_RUNNER;
use crate::{
    all_our_envs::BUILDX_CONFIG,
    coloring::Paint,
    green::Green,
    redact::Redacted,
    remote::Pooled,
    runner::{RUNNERS, Runner},
    supergreen::all_envs,
    sys::{Allocated, fs},
};

/// What our XDG dirs are made of, see `dirs::our_dirs`
const XDG_CACHE: &str = "$XDG_CACHE_HOME/cargo-green";

pub(super) const XDG_CACHE_RUN: &str = "$XDG_CACHE_HOME/cargo-green/run";

pub(super) const XDG_RUNTIME: &str = "$XDG_RUNTIME_DIR/cargo-green";

/// What builds will be configured with and what every runner call gets: runner, builder, pool.
#[must_use]
pub(super) fn envs_in_use(paint: &Paint, green: &Green) -> String {
    let mut settings = String::new();
    for (var, _, val) in all_envs(green) {
        if let Some(val) = val.filter(|val| !val.is_empty()) {
            settings.push_str(&format!("{:?}\n", Redacted(var, &val)));
        }
    }
    let mut sections = vec![paint.section("Settings in use", &settings)];

    let runner = green.runner;
    if runner.is_none() {
        let skipped = format!("skipped ({NO_RUNNER})");
        for title in ["Runner envs", "Pool socket", "Buildx config"] {
            sections.push(paint.section(title, &skipped));
        }
        return sections.join("\n");
    }

    let envs = match green.cmd() {
        Ok(cmd) => cmd.envs_redacted(&[]).join("\n"),
        Err(e) => format!("unavailable: {e:#}"),
    };
    sections.push(paint.section(&format!("Envs of every {runner} call"), &envs));

    sections.push(if let Some(Pooled { var, addr }) = &green.pool {
        paint.section("Pool socket", &format!("{addr} (${var})"))
    } else {
        paint.section("Pool socket (none)", "")
    });
    sections.push(paint.section("Buildx config", &buildx_config_in_use(green)));
    sections.join("\n")
}

/// Which `$BUILDX_CONFIG` docker calls get: it decides what builders exist and how they get dialed
#[must_use]
fn buildx_config_in_use(green: &Green) -> String {
    match (&green.buildx_config, green.runner_env(BUILDX_CONFIG!())) {
        (Some(shadow), _) => {
            format!("{shadow} ({BUILDX_CONFIG}: a copy of yours, going through the pool)")
        }
        (None, Some(yours)) if !yours.is_empty() => {
            format!("{yours} ({BUILDX_CONFIG}: as you set it)")
        }
        (None, _) => "buildx's default".to_owned(),
    }
}

/// Runners this host has, with their versions
pub(super) async fn runners(paint: &Paint, in_use: Runner) -> String {
    let mut text = String::new();
    for runner in RUNNERS {
        if runner.is_none() {
            continue;
        }
        let name = runner.to_string();
        let marker = if *runner == in_use { " (in use)" } else { "" };
        let Ok(exe) = which::which(&name) else {
            text.push_str(&format!("{name}{marker}: not found\n"));
            continue;
        };
        text.push_str(&format!("{name}{marker}: {exe:?}\n"));
        for args in [&["--version"][..], &["buildx", "version"][..]] {
            if let Some(line) = first_stdout_line_of(&exe, args).await {
                text.push_str(&format!("  {line}\n"));
            }
        }
    }
    paint.section("Runners", &text)
}

async fn first_stdout_line_of(exe: &PathBuf, args: &[&str]) -> Option<String> {
    Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8_lossy(&out.stdout).lines().next().map(ToOwned::to_owned))
}

/// Sizes of what's in `root`: each entry's, in total, and as shares of the disk it is on.
pub(super) async fn cache_sizes(paint: &Paint, root: &Utf8Path) -> Result<String> {
    let mut entries = fs().read_dir(root).map_err(|e| anyhow!("Failed `ls {root}`: {e}"))?;
    if entries.is_empty() {
        return Ok(format!("Cache sizes in {root}: empty\n"));
    }
    entries.sort();

    // NOTE: stays around when some files can't be read, having measured all it could
    let (sizes, complete) = spawn_blocking(move || disk_usage(&entries))
        .await
        .map_err(|e| anyhow!("Failed measuring {root}: {e}"))?;
    let StatVfs { f_blocks, f_frsize, .. } =
        statvfs(root.as_std_path()).map_err(|e| anyhow!("Failed `df {root}`: {e}"))?;
    let disk = f_blocks * f_frsize / 1024;
    Ok(render_sizes(paint, root, &sizes, disk, complete))
}

/// What `du -skc` tells: KiB used on disk for each paths, not following symlinks
/// and counting hard links once, then their total.
/// Along with whether all of it could be read.
#[must_use]
fn disk_usage(paths: &[Utf8PathBuf]) -> (Vec<(u64, String)>, bool) {
    let (mut seen, mut complete) = (HashSet::new(), true);
    let mut sizes: Vec<_> = paths
        .iter()
        .map(|path| {
            let kib = bytes_used(path, &mut seen, &mut complete).div_ceil(1024);
            (kib, path.to_string())
        })
        .collect();
    let total = sizes.iter().map(|(kib, _)| kib).sum();
    sizes.push((total, "total".to_owned()));
    (sizes, complete)
}

/// Bytes allocated to `path` and what's under it. `seen` holds the inodes of hard links met so far.
fn bytes_used(path: &Utf8Path, seen: &mut HashSet<(u64, u64)>, complete: &mut bool) -> u64 {
    let Ok(Allocated { bytes, is_dir, hard_linked }) = fs().allocated(path) else {
        *complete = false;
        return 0;
    };
    if hard_linked.is_some_and(|file| !seen.insert(file)) {
        return 0;
    }
    let mut bytes = bytes;
    if is_dir {
        match fs().read_dir(path) {
            Ok(entries) => {
                for entry in entries {
                    bytes += bytes_used(&entry, seen, complete);
                }
            }
            Err(_) => *complete = false,
        }
    }
    bytes
}

/// Like `du -h`
#[must_use]
fn human_size(kib: u64) -> String {
    if kib == 0 {
        return "0".to_owned();
    }
    let mut size: f64 = kib as _;
    let mut units = ["K", "M", "G", "T", "P"].into_iter().peekable();
    let mut unit = "K";
    while let Some(next) = units.next() {
        unit = next;
        if size < 1024.0 || units.peek().is_none() {
            break;
        }
        size /= 1024.0;
    }
    format!("{size:.1}{unit}")
}

#[must_use]
fn render_sizes(
    paint: &Paint,
    root: &Utf8Path,
    sizes: &[(u64, String)],
    disk: u64,
    complete: bool,
) -> String {
    let mut text = String::new();
    let prefix = format!("{root}/");
    for (size, path) in sizes {
        let share = 100.0 * *size as f64 / disk.max(1) as f64;
        let name = path.strip_prefix(&prefix).unwrap_or(path);
        text.push_str(&format!("{:>8} {share:>6.2}%  {name}\n", human_size(*size)));
    }
    if !complete {
        text.push_str("(undercounted: some of it couldn't be read)\n");
    }
    paint.section(
        &format!("Cache sizes in {root} ({XDG_CACHE}, on a {} disk)", human_size(disk)),
        &text,
    )
}

/// Names of the sockets in `dir`, sorted
pub(super) fn sockets_in(dir: &Utf8Path) -> Result<Vec<String>> {
    let mut names = vec![];
    for entry in fs().read_dir(dir).map_err(|e| anyhow!("Failed `ls {dir}`: {e}"))? {
        if let Some(name) = entry.file_name().filter(|_| fs().is_socket(&entry)) {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod i_am_host {
    use anyhow::Result;
    use camino::{Utf8Path, Utf8PathBuf};

    use crate::{
        all_our_envs::BUILDX_CONFIG,
        coloring::Paint,
        doctor::{
            host::{buildx_config_in_use, disk_usage, human_size, render_sizes},
            sockets_in,
        },
        green::Green,
        sys::{Fs, Sys, fake::FakeFs},
    };

    const WIDE: usize = 10_000;

    #[test]
    fn disk_usage_is_du_s() -> Result<()> {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        let (a, b) = (Utf8PathBuf::from("/a"), Utf8PathBuf::from("/b"));
        fs.file(a.join("sub/f"), "x".repeat(64 * 1024));
        fs.mkdir(&b);
        fs.hard_link(&a.join("sub/f"), &b.join("same-f"))?;
        fs.symlink_file(&a.join("sub/f"), &b.join("not-followed"))?;
        let missing = Utf8PathBuf::from("/missing");

        let (sizes, complete) = disk_usage(&[a, b, missing]);
        let kib: Vec<_> = sizes.iter().map(|(kib, _)| *kib).collect();
        assert!(kib[0] >= 64, "{sizes:?}");
        assert!(kib[1] < 64, "hard link counted once: {sizes:?}");
        assert_eq!(kib[2], 0);
        assert_eq!(kib[3], kib[0] + kib[1] + kib[2]);
        assert_eq!(sizes[3].1, "total");
        assert!(!complete, "{missing} doesn't exist", missing = sizes[2].1);
        Ok(())
    }

    #[test]
    fn sizes_render() {
        let sizes = vec![
            (1_048_576, "/c/results".to_owned()),
            (0, "/c/buildkit".to_owned()),
            (1_048_576, "total".to_owned()),
        ];
        let disk = 4_194_304;

        let paint = Paint::plain(WIDE);
        let text = render_sizes(&paint, Utf8Path::new("/c"), &sizes, disk, true);
        assert_eq!(
            text,
            "Cache sizes in /c ($XDG_CACHE_HOME/cargo-green, on a 4.0G disk)

      1.0G  25.00%  results
         0   0.00%  buildkit
      1.0G  25.00%  total
"
        );
        assert!(
            render_sizes(&paint, Utf8Path::new("/c"), &sizes, disk, false).contains("undercounted")
        );
    }

    #[test]
    fn says_which_buildx_config_docker_calls_get() {
        let mut green = Green::default();
        assert_eq!(buildx_config_in_use(&green), "buildx's default");

        green.with_env([(BUILDX_CONFIG!(), "/mine")]);
        assert_eq!(buildx_config_in_use(&green), format!("/mine ({BUILDX_CONFIG}: as you set it)"));

        green.buildx_config = Some("/run/shadow".into());
        let text = buildx_config_in_use(&green);
        assert!(text.starts_with("/run/shadow ($BUILDX_CONFIG: a copy of yours"), "{text}");
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human_size(0), "0");
        assert_eq!(human_size(12), "12.0K");
        assert_eq!(human_size(2048), "2.0M");
        assert_eq!(human_size(5 * 1024 * 1024 * 1024), "5.0T");
    }

    #[test]
    fn sockets_get_listed() -> Result<()> {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.file("/run/not-a-socket", "");
        assert!(sockets_in("/run".into())?.is_empty());
        fs.socket("/run/b.sock");
        fs.socket("/run/a.sock");
        assert_eq!(sockets_in("/run".into())?, ["a.sock", "b.sock"]);
        assert!(sockets_in("/nope".into()).is_err());
        Ok(())
    }
}
