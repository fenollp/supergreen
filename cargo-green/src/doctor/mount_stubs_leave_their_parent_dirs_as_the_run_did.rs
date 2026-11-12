use anyhow::{Result, ensure};

use super::{
    Check,
    buildkit::{IMAGE, OURS, build_tarball, exported, ours, text_of},
};
use crate::green::Green;

pub(super) const MOUNT_STUBS: Check = Check {
    name: "mount stubs get cleaned up",
    expected: "a RUN's mount points leave no entries, and a parent dir the RUN touched keeps that touch (moby/buildkit#3148)",
    why: "BuildKit creates empty placeholders for what a RUN mounts, then removes them. If they stayed behind, these empty entries and their timestamps would end up in layers, making them differ from one build to the next.",
    fixes: &[],
    builder: true,
};

/// A RUN's mounts need mount points: BuildKit makes the missing ones (stubs) as the RUN starts,
/// bumping their parent dir's mtime, then removes them once it's done, restoring that parent's
/// mtime as it was just before (see `MountStubsCleaner`, moby/buildkit#3148). So:
/// * stubs leave no entries behind;
/// * a parent dir left alone by the RUN keeps the mtime the stub's creation gave it: when the RUN
///   started (`bitrepro` RUNs, mounting all over their out dir, touch it);
/// * a parent dir the RUN touched keeps that touch.
pub(super) async fn mount_stubs_leave_their_parent_dirs_as_the_run_did(
    green: &mut Green,
) -> Result<()> {
    let containerfile = ours(
        green,
        &format!(
            r#"FROM {IMAGE} AS base-{OURS}
RUN mkdir -p /left /touched && touch -d @1000 /left /touched && echo x >/f
FROM base-{OURS} AS run-{OURS}
RUN --mount=type=tmpfs,dst=/left/stub/deeper \
    --mount=from=base-{OURS},source=/f,dst=/touched/f \
  touch -d @2000 /touched
FROM run-{OURS} AS check-{OURS}
RUN mkdir /o && ls -A /left /touched >/o/ls && stat -c %Y /left >/o/left && stat -c %Y /touched >/o/touched
FROM scratch AS out-{OURS}
COPY --link --from=check-{OURS} /o /
"#
        ),
    );
    let target = format!("out-{OURS}");
    let (tarball, progress) = build_tarball(green, &containerfile, &target, &[], true).await?;
    progress.some_ran()?;
    let exported = exported(&tarball, true).await?;

    let ls = text_of(&exported, "ls")?;
    ensure!(ls == "/left:\n\n/touched:\n", "stubs are still there: {ls:?}");
    let left: u64 = text_of(&exported, "left")?.trim().parse()?;
    ensure!(left > 1000, "stub creation didn't bump its parent's mtime: {left}");
    let touched = text_of(&exported, "touched")?;
    ensure!(touched == "2000\n", "the RUN's touch wasn't kept: {touched:?}");
    Ok(())
}
