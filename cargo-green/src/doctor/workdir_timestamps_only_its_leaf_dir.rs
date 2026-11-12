use std::collections::BTreeMap;

use anyhow::{Result, anyhow, ensure};

use super::{
    Check,
    buildkit::{IMAGE, OURS, build_tarball, exported, ours, text_of},
};
use crate::{build::SOURCE_DATE_EPOCH, green::Green};

pub(super) const WORKDIR_LEAF: Check = Check {
    name: "WORKDIR only timestamps its leaf dir",
    expected: "the parents WORKDIR creates get the time it ran at, unlike the dir itself",
    why: "WORKDIR gives the dir it creates a fixed timestamp, but the parent dirs it creates along the way get the time of the build. That would make layers differ from one build to the next, so we create these parents beforehand. If BuildKit fixes this, that workaround can go.",
    fixes: &[
        "Good news if parents now get $SOURCE_DATE_EPOCH too: https://github.com/moby/buildkit/pull/7382 or https://github.com/tonistiigi/fsutil/pull/294 landed. Please report it, so our workaround can go",
    ],
    builder: true,
};

/// Given the global `ARG`, WORKDIR timestamps the dir it creates with `$SOURCE_DATE_EPOCH`, but
/// only that one: the parents it creates along the way get the time it ran at. Our WORKDIRs' parents
/// are thus best created beforehand (or exist in the base image).
///
/// Should fail once these land: [file: set the timestamp of all directories created by mkdir -p](https://github.com/moby/buildkit/pull/7382)
/// and [copy: keep a fixed timestamp on the directories a copy creates](https://github.com/tonistiigi/fsutil/pull/294).
pub(super) async fn workdir_timestamps_only_its_leaf_dir(green: &mut Green) -> Result<()> {
    let containerfile = ours(
        green,
        &format!(
            r#"FROM {IMAGE} AS run-{OURS}
WORKDIR /w/x/y
RUN mkdir /o && stat -c '%Y %n' /w /w/x /w/x/y >/o/mtimes
FROM scratch AS out-{OURS}
COPY --link --from=run-{OURS} /o /
"#
        ),
    );
    let target = format!("out-{OURS}");
    let (tarball, progress) = build_tarball(green, &containerfile, &target, &[], true).await?;
    progress.some_ran()?;
    let exported = exported(&tarball, true).await?;

    let lines = text_of(&exported, "mtimes")?;
    let mut mtimes = BTreeMap::new();
    for line in lines.lines() {
        let (mtime, dir) =
            line.split_once(' ').ok_or_else(|| anyhow!("Bad stat line: {line:?}"))?;
        let _ = mtimes.insert(dir.to_owned(), mtime.parse::<u64>()?);
    }
    let mtime = |dir: &str| mtimes.get(dir).copied().ok_or_else(|| anyhow!("No {dir}: {lines:?}"));
    ensure!(mtime("/w/x/y")? == SOURCE_DATE_EPOCH, "leaf mtime: {mtimes:?}");
    ensure!(mtime("/w/x")? > SOURCE_DATE_EPOCH, "parent mtime: {mtimes:?}");
    ensure!(mtime("/w")? > SOURCE_DATE_EPOCH, "grandparent mtime: {mtimes:?}");
    Ok(())
}
