use std::time::Duration;

use anyhow::{Result, ensure};

use super::{
    Check,
    buildkit::{IMAGE, OURS, build, mtime_of, nonce, without_global_arg},
};
use crate::{build::SOURCE_DATE_EPOCH, green::Green};

pub(super) const COPY_REUSE: Check = Check {
    name: "COPY's cache key ignores mtimes",
    expected: "a COPY of what a RUN rewrote identically comes out of the build cache, mtimes included",
    why: "A COPY can come out of the build cache with the timestamps of an earlier build. That's why setting timestamps within RUNs isn't enough and why we need the tar exporter to reset them. Nothing breaks if this stops being true, but we could then simplify.",
    fixes: &["If only this one fails, please report it: we may be able to simplify things"],
    builder: true,
};

/// A COPY's cache key is made of its sources' contents, not of their mtimes. So when a RUN runs
/// again (here: its command changed) yet writes the same contents, the COPY of what it wrote comes
/// out of the build cache: as the build that first made it left it, timestamps included.
///
/// So timestamps of what an output stage holds may come from another, earlier build: results
/// can't do without the tar exporter rewriting them (see the check above), however thoroughly
/// RUNs touch what they write.
pub(super) async fn copy_reuses_its_cached_output_whatever_its_sources_mtimes(
    green: &mut Green,
) -> Result<()> {
    let nonce = nonce()?;
    // No global ARG: timestamps go through as they are
    let syntax = without_global_arg(green, "");
    // As our output stages copy what RUNs wrote
    let containerfile = |variant: &str| {
        format!(
            r#"{syntax}FROM {IMAGE} AS run-{OURS}
RUN mkdir -p /t/deps/sub && echo {nonce} >/t/deps/sub/f && echo {variant} >/dev/null
FROM scratch AS out-{OURS}
COPY --link --parents --from=run-{OURS} /t/./deps /
"#
        )
    };
    let target = format!("out-{OURS}");

    let (first, progress) = build(green, &containerfile("a"), &target, &[], false).await?;
    progress.some_ran()?;
    let mtime = mtime_of(&first, "deps/sub")?;
    ensure!(mtime > SOURCE_DATE_EPOCH, "deps/sub got this mtime: {mtime}");

    // Later on, the RUN's files and dirs get newer mtimes
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let (second, progress) = build(green, &containerfile("b"), &target, &[], false).await?;
    progress.some_ran().map_err(|e| e.context("the RUN was to run again"))?;
    ensure!(second == first, "the COPY's output isn't the first build's: {second:?} vs {first:?}");
    Ok(())
}
