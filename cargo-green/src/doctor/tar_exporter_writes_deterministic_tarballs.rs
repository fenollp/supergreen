use anyhow::{Result, ensure};
use tokio::io::BufReader;
use tokio_stream::StreamExt;
use tokio_tar::Archive as TarArchive;

use super::{
    Check,
    buildkit::{IMAGE, OURS, build_tarball, exported, ours},
};
use crate::{build::STDOUT, green::Green};

pub(super) const DETERMINISTIC_TARBALLS: Check = Check {
    name: "tar exporter is deterministic",
    expected: "same contents give the same bytes, whatever their creation order and timestamps",
    why: "Results are the tarball BuildKit gives us, with an entry of ours added at its end. If the same files could give different bytes, identical builds would give different results. Adding our entry also relies on how that tarball ends.",
    fixes: &[],
    builder: true,
};

/// The tar exporter writes the same bytes for the same contents, however and whenever they came to
/// be. As read in BuildKit v0.34's exporter/tar and fsutil's `WriteTar`, using Go's archive/tar:
/// * entries come in lexical order (`fs.WalkDir`), not creation order;
/// * timestamps are rewritten (see above), and headers hold neither atime nor ctime;
/// * user and group names are left empty (only IDs, from the snapshot);
/// * PAX records (xattrs) are written with sorted keys;
/// * a hardlink is the second path met (in that order) to an inode;
/// * it ends with exactly two zeroed blocks: where results append md.toml.
///
/// Results' bit-reproducibility relies on this, as does our appending to them.
pub(super) async fn tar_exporter_writes_deterministic_tarballs(green: &mut Green) -> Result<()> {
    // The same contents, made in another order and with other timestamps on each run
    let containerfile = |green: &Green, order: &str, mtime: u32| {
        ours(
            green,
            &format!(
                r#"FROM {IMAGE} AS run-{OURS}
RUN mkdir -p /o/d/e /o/empty && cd /o/d && for f in {order}; do echo $f >$f && touch -d @{mtime} $f; done \
 && chmod 0755 b && ln a hard && ln -s a soft && : >e/0 && echo out >/{STDOUT}
FROM scratch AS out-{OURS}
COPY --link --parents --from=run-{OURS} /./o /./{STDOUT} /
"#
            ),
        )
    };
    let target = format!("out-{OURS}");

    let mut tarballs = vec![];
    for (order, mtime) in [("a b c", 1), ("c b a", 2_000_000_000), ("b c a", 3)] {
        let containerfile = containerfile(green, order, mtime);
        let (tarball, progress) = build_tarball(green, &containerfile, &target, &[], true).await?;
        progress.some_ran()?;
        tarballs.push(tarball);
    }

    let tarball = &tarballs[0];
    ensure!(tarballs.iter().all(|t| t == tarball), "tarballs differ");

    let names = exported(tarball, true).await?.into_keys().collect::<Vec<_>>(); // Checks each header
    let expected = [
        "1", "o", "o/d", "o/d/a", "o/d/b", "o/d/c", "o/d/e", "o/d/e/0", "o/d/hard", "o/d/soft",
        "o/empty",
    ];
    ensure!(names == expected, "tarball entries: {names:?}");

    let mut links = vec![];
    let mut ar = TarArchive::new(BufReader::new(&tarball[..]));
    let mut entries = ar.entries()?;
    while let Some(entry) = entries.next().await {
        let f = entry?;
        let (kind, path) = (f.header().entry_type(), f.path()?.to_string_lossy().into_owned());
        if kind.is_hard_link() || kind.is_symlink() {
            let target = f.link_name()?.map(|t| t.to_string_lossy().into_owned());
            links.push((path, kind.is_hard_link(), target));
        }
    }
    let expected = [
        ("o/d/hard".to_owned(), true, Some("o/d/a".to_owned())),
        ("o/d/soft".to_owned(), false, Some("a".to_owned())),
    ];
    ensure!(links == expected, "tarball links: {links:?}");

    ensure!(tarball.len().is_multiple_of(512), "tarball is {} bytes long", tarball.len());
    let end = tarball.len().saturating_sub(1024);
    ensure!(tarball[end..].iter().all(|&b| b == 0), "tarball doesn't end with two zeroed blocks");
    Ok(())
}
