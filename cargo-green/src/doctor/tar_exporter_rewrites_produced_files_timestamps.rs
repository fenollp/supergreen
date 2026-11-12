use anyhow::{Result, ensure};

use super::{
    Check,
    buildkit::{IMAGE, OURS, build, mtime_of, mtimes, ours, without_global_arg},
};
use crate::{
    build::{SOURCE_DATE_EPOCH, STDOUT},
    green::Green,
};

pub(super) const TAR_EXPORTER_REWRITES: Check = Check {
    name: "tar exporter rewrites timestamps",
    expected: "given the global ARG, produced files get exported with $SOURCE_DATE_EPOCH as mtime",
    why: "Build outputs reach us as a tarball, and its files carry the time they were built at unless BuildKit resets it. Resetting it is what makes the same code give the exact same result, on any machine and at any time.",
    fixes: &[],
    builder: true,
};

/// Produced files reach results with `$SOURCE_DATE_EPOCH` timestamps: the tar exporter rewrites
/// them (rather than clamping them: see `early`) given the global `ARG` every Containerfile starts
/// with. Defined within a stage, it isn't enough. Results' bit-reproducibility relies on this.
pub(super) async fn tar_exporter_rewrites_produced_files_timestamps(
    green: &mut Green,
) -> Result<()> {
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
    let target = format!("out-{OURS}");

    let containerfile = ours(green, &stages(""));
    let (rewritten, _) = build(green, &containerfile, &target, &[], true).await?;
    let got = mtimes(&rewritten);
    let expected = ["1", "deps", "deps/early", "deps/late"].map(|f| (f, SOURCE_DATE_EPOCH));
    ensure!(got == expected, "exported mtimes: {got:?}");

    // Within a stage, the RUN's own timestamps go through
    let in_stage = stages(&format!("ARG SOURCE_DATE_EPOCH={SOURCE_DATE_EPOCH}"));
    let containerfile = without_global_arg(green, &in_stage);
    let (untouched, _) = build(green, &containerfile, &target, &[], false).await?;
    let early = mtime_of(&untouched, "deps/early")?;
    ensure!(early == 7, "ARG within a stage changed deps/early's mtime: {early}");
    let late = mtime_of(&untouched, "deps/late")?;
    ensure!(late > SOURCE_DATE_EPOCH, "ARG within a stage changed deps/late's mtime: {late}");
    Ok(())
}
