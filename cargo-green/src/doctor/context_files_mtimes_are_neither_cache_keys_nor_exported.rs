use std::{
    fs::File,
    time::{Duration, SystemTime},
};

use anyhow::{Result, ensure};

use super::{
    Check,
    buildkit::{ContextDir, IMAGE, OURS, build, mtimes, nonce, ours, text_of},
};
use crate::{build::SOURCE_DATE_EPOCH, green::Green};

pub(super) const CONTEXT_MTIMES: Check = Check {
    name: "build contexts' mtimes don't matter",
    expected: "touching a local file doesn't make RUNs mounting it run again, and its mtime isn't exported",
    why: "Your local code gets handed to builds as a build context, and a fresh checkout gives every file a new modification time. If those times counted, a fresh checkout or another machine would build everything again instead of reusing the cache.",
    fixes: &[],
    builder: true,
};

/// Local code reaches builds as a build context, mounted (never copied) into RUNs. Its files'
/// mtimes are the host's, as is: a RUN sees them, yet
/// * they are not part of cache keys: touching a file doesn't make the RUN run again (only
///   changing its contents does), so builds on different hosts or checkouts share cache;
/// * they don't reach results, even when copied over with them (e.g. by a build script): the
///   tar exporter rewrites them, too.
pub(super) async fn context_files_mtimes_are_neither_cache_keys_nor_exported(
    green: &mut Green,
) -> Result<()> {
    let ctx = ContextDir::new("mtimes")?;
    let src = ctx.0.join("lib.rs");
    let nonce = nonce()?;
    std::fs::write(&src, format!("// {nonce}\n"))?;
    let set_mtime = |secs: u64| -> Result<()> {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
        Ok(File::options().write(true).open(&src)?.set_modified(t)?)
    };
    set_mtime(1_000_000)?;

    let name = format!("cwd-{OURS}");
    let containerfile = ours(
        green,
        &format!(
            r#"FROM {IMAGE} AS run-{OURS}
RUN --mount=from={name},source=/lib.rs,dst=/src/lib.rs \
  mkdir /o && stat -c %Y /src/lib.rs >/o/seen && cp -p /src/lib.rs /o/lib.rs
FROM scratch AS out-{OURS}
COPY --link --from=run-{OURS} /o /
"#
        ),
    );
    let target = format!("out-{OURS}");
    let contexts = [(name.as_str(), ctx.0.as_path())];

    let (exported, progress) = build(green, &containerfile, &target, &contexts, true).await?;
    progress.some_ran()?;
    // The RUN saw the host's mtime...
    let seen = text_of(&exported, "seen")?;
    ensure!(seen == "1000000\n", "the RUN saw this mtime: {seen:?}");
    // ...but it doesn't reach results
    let got = mtimes(&exported);
    ensure!(got == ["lib.rs", "seen"].map(|f| (f, SOURCE_DATE_EPOCH)), "exported mtimes: {got:?}");

    // Only the mtime changes: cached
    set_mtime(2_000_000)?;
    let (again, progress) = build(green, &containerfile, &target, &contexts, true).await?;
    progress.all_cached().map_err(|e| e.context("after only touching a file"))?;
    ensure!(again == exported, "after only touching a file, exported something else");

    // Contents change: runs
    std::fs::write(&src, format!("// {nonce} changed\n"))?;
    let (_, progress) = build(green, &containerfile, &target, &contexts, true).await?;
    progress.some_ran().map_err(|e| e.context("after changing a file's contents"))
}
