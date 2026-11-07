//! Properties of BuildKit (and of its Dockerfile frontend) that we rely on: asserted against an
//! actual runner, called with the flags our own builds use, so that upgrading either (or the
//! syntax image) can't silently break what results' and the build cache's reproducibility (or
//! wrappers' reuse of their deps' stages) stands on.
//!
//! They need a BuildKit daemon (wherever `$DOCKER_HOST`, `$BUILDX_BUILDER`, ... point to) and to
//! pull images: they only run when `$CI=true`.

use std::{
    collections::BTreeMap,
    fs::File,
    time::{Duration, SystemTime},
};

use anyhow::{Result, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexSet;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio_stream::StreamExt;
use tokio_tar::Archive as TarArchive;

use crate::{
    build::{SOURCE_DATE_EPOCH, STDOUT},
    cache::result::assert_tarball_header,
    containerfile::Containerfile,
    green::Green,
    image_uri::SYNTAX_IMAGE_LOCKED,
    md::BuildContext,
    progress::{Parser, Progress},
    redact::Vars,
    stage::Stage,
};

const IMAGE: &str = "docker.io/library/busybox:1.37";

/// Suffix of the stages a test builds: what [`Parser`] tells our own steps by.
const OURS: &str = "0123456789abcdef";

#[must_use]
fn enabled() -> bool {
    std::env::var("CI").as_deref() == Ok("true")
}

/// A Containerfile as our builds start theirs, followed by `stages`.
#[must_use]
fn ours(stages: &str) -> String {
    let mut containerfile = Containerfile::with_syntax(&SYNTAX_IMAGE_LOCKED);
    containerfile.push(stages);
    containerfile.as_str().to_owned()
}

/// A fresh dir, for a build context.
fn context_dir(name: &str) -> Result<Utf8PathBuf> {
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_nanos();
    let dir = std::env::temp_dir().join(format!("cargo-green-buildkit-props-{name}-{nanos}"));
    let dir = Utf8PathBuf::try_from(dir)?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// What a build exported, file by file: mtime and contents.
type Exported = BTreeMap<String, (u64, Vec<u8>)>;

/// Builds `target` of `containerfile` with `contexts`, the way a wrapper does (same runner and
/// flags, the tar exporter on STDOUT), and returns what got exported along with the progress.
async fn build(
    containerfile: &str,
    target: &str,
    contexts: &[(&str, &Utf8Path)],
    rewritten: bool,
) -> Result<(Exported, Progress)> {
    let green = Green { env: std::env::vars().collect::<Vars>(), ..Green::default() };
    let target = Stage::new(target)?;
    let contexts: IndexSet<_> = contexts
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

    let mut parser = Parser::new(OURS);
    stderr.lines().for_each(|line| parser.feed(line));

    let mut exported = BTreeMap::new();
    let mut ar = TarArchive::new(BufReader::new(&out.stdout[..]));
    let mut entries = ar.entries()?;
    while let Some(entry) = entries.next().await {
        let mut f = entry?;
        let name = f.path()?.to_string_lossy().trim_end_matches('/').to_owned();
        if rewritten {
            assert_tarball_header(f.header()); // As each result's entries must
        }
        let mtime = f.header().mtime()?;
        let mut data = vec![];
        tokio::io::AsyncReadExt::read_to_end(&mut f, &mut data).await?;
        let _ = exported.insert(name, (mtime, data));
    }
    Ok((exported, parser.finish()))
}

fn mtimes(exported: &Exported) -> Vec<(&str, u64)> {
    exported.iter().map(|(name, (mtime, _))| (name.as_str(), *mtime)).collect()
}

/// Produced files reach results with `$SOURCE_DATE_EPOCH` timestamps: the tar exporter rewrites
/// them (rather than clamping them: see `early`) given the global `ARG` every Containerfile starts
/// with. Defined within a stage, it isn't enough. Results' bit-reproducibility relies on this.
#[tokio::test]
async fn tar_exporter_rewrites_produced_files_timestamps() -> Result<()> {
    if !enabled() {
        return Ok(());
    }

    let stages = |arg: &str| {
        format!(
            r#"FROM {IMAGE} AS run-{OURS}
{arg}
RUN mkdir /deps && echo data >/deps/late && echo data >/deps/early && touch -d @7 /deps/early && echo out >/{STDOUT}
FROM scratch AS out-{OURS}
COPY --link --from=run-{OURS} /deps /deps
COPY --link --from=run-{OURS} /{STDOUT} /
"#
        )
    };

    let (rewritten, _) = build(&ours(&stages("")), &format!("out-{OURS}"), &[], true).await?;
    assert_eq!(
        mtimes(&rewritten),
        [
            ("1", SOURCE_DATE_EPOCH),
            ("deps", SOURCE_DATE_EPOCH),
            ("deps/early", SOURCE_DATE_EPOCH),
            ("deps/late", SOURCE_DATE_EPOCH),
        ]
    );

    // Within a stage, the RUN's own timestamps go through
    let syntax = format!("# syntax={}\n", SYNTAX_IMAGE_LOCKED.noscheme());
    let in_stage = stages(&format!("ARG SOURCE_DATE_EPOCH={SOURCE_DATE_EPOCH}"));
    let (untouched, _) =
        build(&format!("{syntax}{in_stage}"), &format!("out-{OURS}"), &[], false).await?;
    let untouched: BTreeMap<_, _> = mtimes(&untouched).into_iter().collect();
    assert_eq!(untouched.get("deps/early"), Some(&7));
    assert!(untouched["deps/late"] > SOURCE_DATE_EPOCH, "{untouched:?}");
    Ok(())
}

/// Local code reaches builds as a build context, mounted (never copied) into RUNs. Its files'
/// mtimes are the host's, as is: a RUN sees them, yet
/// * they are not part of cache keys: touching a file doesn't make the RUN run again (only
///   changing its contents does), so builds on different hosts or checkouts share cache;
/// * they don't reach results, even when copied over with them (e.g. by a build script): the
///   tar exporter rewrites them, too.
#[tokio::test]
async fn context_files_mtimes_are_neither_cache_keys_nor_exported() -> Result<()> {
    if !enabled() {
        return Ok(());
    }

    let ctx = context_dir("mtimes")?;
    let src = ctx.join("lib.rs");
    // Unique contents: this test's first build can't be cached
    let nonce = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_nanos();
    std::fs::write(&src, format!("// {nonce}\n"))?;
    let set_mtime = |secs: u64| -> Result<()> {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
        Ok(File::options().write(true).open(&src)?.set_modified(t)?)
    };
    set_mtime(1_000_000)?;

    let name = format!("cwd-{OURS}");
    let containerfile = ours(&format!(
        r#"FROM {IMAGE} AS run-{OURS}
RUN --mount=from={name},source=/lib.rs,dst=/src/lib.rs \
  mkdir /o && stat -c %Y /src/lib.rs >/o/seen && cp -p /src/lib.rs /o/lib.rs
FROM scratch AS out-{OURS}
COPY --link --from=run-{OURS} /o /
"#
    ));
    let target = format!("out-{OURS}");
    let contexts = [(name.as_str(), ctx.as_path())];

    let (exported, progress) = build(&containerfile, &target, &contexts, true).await?;
    assert_ne!(progress.own.ran, 0, "{progress}");
    // The RUN saw the host's mtime...
    assert_eq!(exported["seen"].1, b"1000000\n");
    // ...but it doesn't reach results
    assert_eq!(mtimes(&exported), [("lib.rs", SOURCE_DATE_EPOCH), ("seen", SOURCE_DATE_EPOCH)]);

    // Only the mtime changes: cached
    set_mtime(2_000_000)?;
    let (again, progress) = build(&containerfile, &target, &contexts, true).await?;
    assert_eq!(progress.own.ran, 0, "{progress}");
    assert_ne!(progress.own.cached, 0, "{progress}");
    assert_eq!(again, exported);

    // Contents change: runs
    std::fs::write(&src, format!("// {nonce} changed\n"))?;
    let (_, progress) = build(&containerfile, &target, &contexts, true).await?;
    assert_ne!(progress.own.ran, 0, "{progress}");

    std::fs::remove_dir_all(&ctx)?;
    Ok(())
}
