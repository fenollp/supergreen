//! Build results are the artifacts of the runner's `rustc` (and build scripts)
//! invocations bundled as a tarball.

use std::io::Read;

use anyhow::{Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info};
use ruzstd::{
    decoding::StreamingDecoder,
    encoding::{CompressionLevel, compress_to_vec},
};
use tokio::io::AsyncReadExt;
use tokio_stream::StreamExt;
use tokio_tar::{Archive as TarArchive, Builder as TarBuilder, EntryType, Header};

use crate::{build::SOURCE_DATE_EPOCH, dirs::Dirs, stage::Stage, sys::fs};

pub(crate) const RESULT_TAR: &str = "result.tar";
pub(crate) const MD_TOML: &str = "md.toml";

impl Dirs {
    pub(crate) fn result_from_stage(&self, target: &Stage) -> Utf8PathBuf {
        self.results.join(format!("{}.tar.zst", target.bucketed()))
    }

    /// What building this stage's result cost: kept out of it for its bytes not to vary.
    pub(crate) fn stats_from_stage(&self, target: &Stage) -> Utf8PathBuf {
        self.stats.join(format!("{}.toml", target.bucketed()))
    }

    pub(crate) async fn new_result(&self, target: &Stage) -> Result<Option<ResultWriter>> {
        let dst = self.result_from_stage(target);
        if fs().exists(&dst) {
            return Ok(None);
        }
        // NOTE: TOCTOU on dst is okay as long as writing it is atomic
        Ok(Some(ResultWriter { dst, w: TarBuilder::new(vec![]) }))
    }
}

/// An archive, held in memory (as is what goes in it) then compressed in one go
pub(crate) struct ResultWriter {
    w: TarBuilder<Vec<u8>>,
    dst: Utf8PathBuf,
}

impl ResultWriter {
    pub(crate) async fn add_tarball(&mut self, built: &[u8]) -> Result<()> {
        let header = header_for(RESULT_TAR, built.len())?;
        self.w
            .append(&header, built)
            .await
            .map_err(|e| anyhow!("Failed appending tar to result: {e}"))
    }

    /// No-op when another build already stored the result
    pub(crate) async fn finalize(self, md_ser: &str) -> Result<()> {
        let Self { dst, mut w } = self;

        let header = header_for(MD_TOML, md_ser.len())?;
        w.append(&header, md_ser.as_bytes())
            .await
            .map_err(|e| anyhow!("Failed appending Md to result: {e}"))?;

        let tar = w.into_inner().await.map_err(|e| anyhow!("Failed finishing result: {e}"))?;

        if fs().exists(&dst) {
            debug!("{dst} already exists, dropping work");
            return Ok(());
        }
        info!("writing result to {dst}");
        let zst = compress(&tar);
        if let Some(bucket) = dst.parent() {
            fs().create_dir_all(bucket)
                .map_err(|e| anyhow!("Failed to `mkdir -p {bucket}`: {e}"))?;
        }
        fs().write_atomic_bytes(&dst, &zst, 0o644)
            .map_err(|e| anyhow!("Failed writing result {dst}: {e}"))?;
        Ok(())
    }
}

#[must_use]
fn compress(tar: &[u8]) -> Vec<u8> {
    // The only level implemented besides `Uncompressed`: on results, ~3x faster than gzip's
    // default level, though ~50% bigger. No checksum (ruzstd's `hash` feature): less `unsafe`.
    compress_to_vec(tar, CompressionLevel::Fastest)
}

fn decompress(src: &Utf8Path, zst: &[u8]) -> Result<Vec<u8>> {
    let mut tar = vec![];
    StreamingDecoder::new(zst)
        .map_err(|e| anyhow!("Failed decoding {src}: {e}"))?
        .read_to_end(&mut tar)
        .map_err(|e| anyhow!("Failed decompressing {src}: {e}"))?;
    Ok(tar)
}

/// Extracts the named entries of a result in one pass, in the order asked, each empty when absent.
pub(crate) async fn extract<const N: usize>(
    src: &Utf8Path,
    fnames: [&str; N],
) -> Result<[Vec<u8>; N]> {
    let zst = fs().read(src).map_err(|e| anyhow!("Failed reading tarball {src}: {e}"))?;
    let tar = decompress(src, &zst)?;

    let mut inners = [const { Vec::new() }; N];
    let mut ar = TarArchive::new(tar.as_slice());
    let mut entries = ar.entries().map_err(|e| anyhow!("Failed reading {src}: {e}"))?;
    while let Some(entry) = entries.next().await {
        let mut f = entry.map_err(|e| anyhow!("Failed streaming tarball {src}: {e}"))?;
        let name = f
            .path()
            .map_err(|e| anyhow!("Failed decoding {src} entry name: {e}"))?
            .to_string_lossy()
            .to_string();
        let Some(at) = fnames.iter().position(|fname| *fname == name) else { continue };
        let _ = f
            .read_to_end(&mut inners[at])
            .await
            .map_err(|e| anyhow!("Failed extracting {name} from {src}: {e}"))?;
    }
    Ok(inners)
}

pub(crate) fn header_for(fname: &str, len: usize) -> Result<Header> {
    let mut header = Header::new_gnu();
    header.set_path(fname).map_err(|e| anyhow!("Failed setting {fname} path: {e}"))?;
    match len.try_into() {
        Ok(n) => header.set_size(n),
        Err(e) => bail!("tar too big: {e}"),
    }
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(SOURCE_DATE_EPOCH);
    header.set_username("").map_err(|e| anyhow!("Failed setting {fname} username: {e}"))?;
    header.set_groupname("").map_err(|e| anyhow!("Failed setting {fname} groupname: {e}"))?;
    header.set_entry_type(EntryType::Regular);
    header.set_device_major(0).map_err(|e| anyhow!("Failed setting {fname} devmaj: {e}"))?;
    header.set_device_minor(0).map_err(|e| anyhow!("Failed setting {fname} devmin: {e}"))?;
    header.set_mode(0o644);
    header.set_cksum();
    assert_tarball_header(&header);
    Ok(header)
}

pub(crate) fn assert_tarball_header(header: &Header) {
    assert_eq!(header.uid().ok(), Some(0));
    assert_eq!(header.gid().ok(), Some(0));
    assert_eq!(header.mtime().ok(), Some(SOURCE_DATE_EPOCH));
    assert_eq!(header.username(), Ok(Some("")));
    assert_eq!(header.groupname(), Ok(Some("")));
    assert_eq!(header.device_major().ok(), Some(Some(0)));
    assert_eq!(header.device_minor().ok(), Some(Some(0)));
}

#[tokio::test]
async fn roundtripping() -> Result<()> {
    let mut w = TarBuilder::new(vec![]);

    let some_data = vec![10, 10, 10];
    let header = header_for("some.file", some_data.len())?;
    w.append(&header, some_data.as_slice()).await?;

    let zst = compress(&w.into_inner().await?);
    assert_eq!(zst[..4], [0x28, 0xb5, 0x2f, 0xfd]); // zstd's magic number
    let buf = decompress("some.tar.zst".into(), &zst)?;
    let mut r = TarArchive::new(buf.as_slice());

    let mut entries = r.entries()?;
    let entry = entries.next().await.expect("we wrote 1 entry");
    let mut entry = entry?;
    let header_bis = entry.header();
    assert_tarball_header(header_bis);

    assert_eq!(header.as_bytes(), header_bis.as_bytes());
    assert_eq!(header.path().ok(), header_bis.path().ok());
    assert_eq!(header.link_name().ok(), header_bis.link_name().ok());
    assert_eq!(header.raw_entry_size().ok(), header_bis.raw_entry_size().ok());
    assert_eq!(header.raw_file_size().ok(), header_bis.raw_file_size().ok());
    assert_eq!(entry.effective_size(), header.raw_entry_size().unwrap());

    let mut buf = Vec::new();
    entry.read_to_end(&mut buf).await?;
    assert_eq!(buf, some_data);

    Ok(())
}

#[tokio::test]
async fn extracting_entries_in_one_pass() -> Result<()> {
    use crate::sys::{Sys, fake::FakeFs};

    let fakefs = FakeFs::new();
    let _guard = Sys::install(Sys { fs: fakefs.clone(), ..Sys::fake() });
    let dst: Utf8PathBuf = "/results/out-3333333333333333.tar.zst".into();

    let mut w = ResultWriter { dst: dst.clone(), w: TarBuilder::new(vec![]) };
    w.add_tarball(b"not really a tar").await?;
    w.finalize("stamp = 3").await?;
    assert_eq!(fakefs.written(), std::slice::from_ref(&dst));

    let w = ResultWriter { dst: dst.clone(), w: TarBuilder::new(vec![]) };
    w.finalize("not stored").await?;
    assert_eq!(fakefs.written(), std::slice::from_ref(&dst), "another build already stored it");

    let [md, missing, tarball] = extract(&dst, [MD_TOML, "nope", RESULT_TAR]).await?;
    assert_eq!(tarball, b"not really a tar");
    assert_eq!(md, b"stamp = 3");
    assert!(missing.is_empty());
    Ok(())
}

#[test]
fn entries_are_bucketed_by_hash() {
    use crate::{
        md::MdId,
        sys::{Sys, fake::FakeFs},
    };

    let fakefs = FakeFs::new();
    let _guard = Sys::install(Sys { fs: fakefs.clone(), ..Sys::fake() });
    let dirs = Dirs {
        results: "/r".into(),
        stats: "/s".into(),
        buildkit: "/b".into(),
        var_run: "/v".into(),
    };
    let id = MdId::from("eb09e02c579e5fe1");

    let out = Stage::output(id);
    assert_eq!(dirs.result_from_stage(&out), "/r/eb/out-eb09e02c579e5fe1.tar.zst");
    assert_eq!(dirs.stats_from_stage(&out), "/s/eb/out-eb09e02c579e5fe1.toml");

    let cache = dirs.new_runner_cache(&Stage::incremental(id)).unwrap();
    assert_eq!(cache.unwrap(), "/b/eb/inc-eb09e02c579e5fe1");
    assert_eq!(Stage::new("short").unwrap().bucketed(), "short", "no hash: no bucket");
}
