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
use tokio::{fs, io::AsyncReadExt};
use tokio_stream::StreamExt;
use tokio_tar::{Archive as TarArchive, Builder as TarBuilder, EntryType, Header};
use uuid::Uuid;

use crate::{build::SOURCE_DATE_EPOCH, dirs::Dirs, stage::Stage};

/// The runner's own output tarball, as built.
pub(crate) const RESULT_TAR: &str = "result.tar";
/// The crate's `Md`, as of that build (with what that build cost).
pub(crate) const MD_TOML: &str = "md.toml";

impl Dirs {
    pub(crate) fn result_from_stage(&self, target: &Stage) -> Utf8PathBuf {
        self.results.join(format!("{target}.tar.zst"))
    }

    pub(crate) async fn new_result(&self, target: &Stage) -> Result<Option<ResultWriter>> {
        let dst = self.result_from_stage(target);
        let tmp = self.tmp.join(format!("{}.tar.zst", Uuid::new_v4()));
        if dst.exists() {
            return Ok(None);
        }
        // NOTE: TOCTOU on dst is okay as long as `mv` is atomic
        Ok(Some(ResultWriter { tmp, dst, w: TarBuilder::new(vec![]) }))
    }
}

/// An archive, held in memory (as is what goes in it) then compressed in one go
pub(crate) struct ResultWriter {
    w: TarBuilder<Vec<u8>>,
    tmp: Utf8PathBuf,
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

    /// Seals the result, returning what it takes up on disk (0 when another build won).
    pub(crate) async fn finalize(self, md_ser: &str) -> Result<u64> {
        let Self { tmp, dst, mut w } = self;

        let header = header_for(MD_TOML, md_ser.len())?;
        w.append(&header, md_ser.as_bytes())
            .await
            .map_err(|e| anyhow!("Failed appending Md to result: {e}"))?;

        let tar = w.into_inner().await.map_err(|e| anyhow!("Failed finishing result: {e}"))?;

        if dst.exists() {
            debug!("{dst} already exists, dropping work");
            return Ok(0);
        }
        debug!("writing result to {tmp}");
        let zst = compress(&tar);
        let len = zst.len() as u64;
        fs::write(&tmp, zst).await.map_err(|e| anyhow!("Failed writing result {tmp}: {e}"))?;
        info!("moving result to {dst}");
        fs::rename(&tmp, &dst).await.map_err(|e| anyhow!("Failed `mv {tmp} {dst}`: {e}"))?;
        Ok(len)
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
    let zst = fs::read(&src).await.map_err(|e| anyhow!("Failed reading tarball {src}: {e}"))?;
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
    let dir: Utf8PathBuf = std::env::temp_dir().try_into()?;
    let dir = dir.join(format!("result-{}", Uuid::new_v4()));
    fs::create_dir_all(&dir).await?;
    let dst = dir.join("out-3333333333333333.tar.zst");

    let mut w = ResultWriter { tmp: dir.join("tmp"), dst: dst.clone(), w: TarBuilder::new(vec![]) };
    w.add_tarball(b"not really a tar").await?;
    let stored = w.finalize("stamp = 3").await?;
    assert_eq!(stored, fs::metadata(&dst).await?.len());

    let [md, missing, tarball] = extract(&dst, [MD_TOML, "nope", RESULT_TAR]).await?;
    assert_eq!(tarball, b"not really a tar");
    assert_eq!(md, b"stamp = 3");
    assert!(missing.is_empty());

    fs::remove_dir_all(&dir).await?;
    Ok(())
}
