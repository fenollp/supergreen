use std::{
    collections::{BTreeMap, BTreeSet},
    time::SystemTime,
};

use anyhow::{Result, anyhow, bail, ensure};
use camino::{Utf8Path, Utf8PathBuf};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio_stream::StreamExt;
use tokio_tar::Archive as TarArchive;

use crate::{
    cache::result::unexpected_header_field, containerfile::Containerfile, green::Green,
    md::BuildContext, stage::Stage,
};

pub(super) const IMAGE: &str = "docker.io/library/busybox:1.37";

/// Suffix of the stages a check builds: what [`Progress`] tells our own steps by.
pub(super) const OURS: &str = "0123456789abcdef";

/// A Containerfile as our builds start theirs, followed by `stages`.
#[must_use]
pub(super) fn ours(green: &Green, stages: &str) -> String {
    let mut containerfile = Containerfile::with_syntax(&green.syntax);
    containerfile.push(stages);
    containerfile.as_str().to_owned()
}

/// A Containerfile whose ARGs are all `stages`' own: without our global one
#[must_use]
pub(super) fn without_global_arg(green: &Green, stages: &str) -> String {
    format!("# syntax={}\n{stages}", green.syntax.noscheme())
}

/// A fresh dir, for a build context: gone once dropped.
pub(super) struct ContextDir(pub(super) Utf8PathBuf);

impl ContextDir {
    pub(super) fn new(name: &str) -> Result<Self> {
        let dir = std::env::temp_dir().join(format!("cargo-green-doctor-{name}-{}", nonce()?));
        let dir = Utf8PathBuf::try_from(dir)?;
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }
}

impl Drop for ContextDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// For unique contents: a check's first build then can't be cached
pub(super) fn nonce() -> Result<u128> {
    Ok(SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_nanos())
}

/// What a build exported, file by file: mtime and contents.
/// A BTreeMap, not an IndexMap: compared and listed by name, whatever the tarball's order.
pub(super) type Exported = BTreeMap<String, (u64, Vec<u8>)>;

/// Builds `target` of `containerfile` with `contexts`, the way a wrapper does (same runner and
/// flags, the tar exporter on STDOUT), and returns what got exported along with the progress.
pub(super) async fn build(
    green: &mut Green,
    containerfile: &str,
    target: &str,
    contexts: &[(&str, &Utf8Path)],
    rewritten: bool,
) -> Result<(Exported, Progress)> {
    let (tarball, progress) = build_tarball(green, containerfile, target, contexts, false).await?;
    Ok((exported(&tarball, rewritten).await?, progress))
}

/// Same as [`build`], returning the tarball as the tar exporter wrote it. With `no_cache`, every
/// step runs again (as with the `repro` experiment).
pub(super) async fn build_tarball(
    green: &mut Green,
    containerfile: &str,
    target: &str,
    contexts: &[(&str, &Utf8Path)],
    no_cache: bool,
) -> Result<(Vec<u8>, Progress)> {
    green.experiment = if no_cache { vec!["repro".to_owned()] } else { vec![] };
    let target = Stage::new(target)?;
    let contexts: BTreeSet<_> = contexts
        .iter()
        .map(|(name, uri)| Ok(BuildContext { name: Stage::new(name)?, uri: uri.to_path_buf() }))
        .collect::<Result<_>>()?;

    let mut cmd = green.cmd()?;
    cmd.arg("build");
    let (_call, _envs) = green.with_docker_args(
        &mut cmd,
        "-".into(),
        &target,
        &contexts,
        Some("/out".into()),
        None,
        false,
    );
    let mut child = cmd.spawn()?;
    let mut stdin = child.stdin.take().expect("piped");
    stdin.write_all(containerfile.as_bytes()).await?;
    drop(stdin);
    let out = child.wait_with_output().await?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        return Err(anyhow!("Build failed: {stderr}"));
    }

    Ok((out.stdout, Progress::of(&stderr)))
}

/// Reads an exported tarball, checking each entry's header as results' if `rewritten`.
pub(super) async fn exported(tarball: &[u8], rewritten: bool) -> Result<Exported> {
    let mut exported = BTreeMap::new();
    let mut ar = TarArchive::new(BufReader::new(tarball));
    let mut entries = ar.entries()?;
    while let Some(entry) = entries.next().await {
        let mut f = entry?;
        let name = f.path()?.to_string_lossy().trim_end_matches('/').to_owned();
        if rewritten && let Some(field) = unexpected_header_field(f.header()) {
            bail!("Unexpected {field} in the tar header of {name}: {:?}", f.header())
        }
        let mtime = f.header().mtime()?;
        let mut data = vec![];
        tokio::io::AsyncReadExt::read_to_end(&mut f, &mut data).await?;
        let _ = exported.insert(name, (mtime, data));
    }
    Ok(exported)
}

/// How many steps of our own stages (named `*-{OURS}`) were cached, how many ran: as BuildKit's
/// plain progress (on STDERR, not a TTY) says.
#[derive(Debug, Default)]
pub(super) struct Progress {
    cached: u32,
    ran: u32,
    log: String,
}

impl Progress {
    #[must_use]
    pub(super) fn of(stderr: &str) -> Self {
        let suffix = format!("-{OURS}");
        let mut progress = Self { log: stderr.to_owned(), ..Self::default() };
        // BTreeSets, not IndexSets, yet only ever asked what they hold: either would do
        let mut vertices = BTreeSet::new(); // Their numbers: each's first line is its name
        let mut ours = BTreeSet::new();
        for line in stderr.lines() {
            let Some((num, rest)) = line.strip_prefix('#').and_then(|line| line.split_once(' '))
            else {
                continue;
            };
            let Ok(num) = num.parse::<u32>() else { continue };
            if vertices.insert(num) {
                // [stage-name i/n] INSTR (resolving FROM's image: always DONE, never CACHED)
                if let Some((stage, instr)) = rest.strip_prefix('[').and_then(|r| r.split_once(' '))
                    && stage.ends_with(&suffix)
                    && !instr.split_once("] ").is_some_and(|(_, i)| i.starts_with("FROM "))
                {
                    let _ = ours.insert(num);
                }
            } else if ours.contains(&num) {
                if rest == "CACHED" {
                    progress.cached += 1;
                } else if rest.starts_with("DONE ") {
                    progress.ran += 1;
                }
            }
        }
        progress
    }

    pub(super) fn some_ran(&self) -> Result<()> {
        let Self { cached, ran, log } = self;
        ensure!(*ran != 0, "no steps ran ({cached} cached), build log:\n{log}");
        Ok(())
    }

    pub(super) fn all_cached(&self) -> Result<()> {
        let Self { cached, ran, log } = self;
        ensure!(*ran == 0 && *cached != 0, "{ran} steps ran, {cached} cached, build log:\n{log}");
        Ok(())
    }
}

pub(super) fn mtimes(exported: &Exported) -> Vec<(&str, u64)> {
    exported.iter().map(|(name, (mtime, _))| (name.as_str(), *mtime)).collect()
}

pub(super) fn mtime_of(exported: &Exported, name: &str) -> Result<u64> {
    Ok(exported.get(name).ok_or_else(|| anyhow!("{name} wasn't exported: {exported:?}"))?.0)
}

pub(super) fn text_of(exported: &Exported, name: &str) -> Result<String> {
    let (_, data) = exported.get(name).ok_or_else(|| anyhow!("{name} wasn't exported"))?;
    Ok(String::from_utf8_lossy(data).into_owned())
}

#[cfg(test)]
mod busybox_in_the_machine {
    use super::Progress;

    #[test]
    fn progress_tells_our_steps_apart() {
        let stderr = "#1 [internal] load build definition
#1 DONE 0.0s
#2 [run-0123456789abcdef 1/2] FROM docker.io/library/busybox:1.37
#2 DONE 0.1s
#3 [run-0123456789abcdef 2/2] RUN true
#3 CACHED
#4 [out-0123456789abcdef 1/1] COPY --link /o /
#4 DONE 0.2s
";
        let Progress { cached, ran, log } = Progress::of(stderr);
        assert_eq!((cached, ran), (1, 1));
        assert_eq!(log, stderr);
    }
}
