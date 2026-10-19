//! Build results are the artifacts of the runner's `rustc` (and build scripts)
//! invocations bundled as a tarball.

use std::io::Read;

use anyhow::{Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info, warn};
use ruzstd::{
    decoding::StreamingDecoder,
    encoding::{CompressionLevel, compress_to_vec},
};
use serde::{Deserialize, Serialize};
use tokio::{fs, io::AsyncReadExt};
use tokio_stream::StreamExt;
use tokio_tar::{Archive as TarArchive, Builder as TarBuilder, EntryType, Header};
use uuid::Uuid;

use crate::{build::SOURCE_DATE_EPOCH, dirs::Dirs, stage::Stage, sys::fs as sys_fs};

/// What a stored result knows about itself, kept beside its tarball.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Meta {
    /// How long the runner took to produce it, in milliseconds.
    pub(crate) took_ms: u64,
}

impl Dirs {
    /// `key` is the [`crate::containerfile::result_key`] of the build `target` is part of.
    pub(crate) fn result_from_stage(&self, target: &Stage, key: &str) -> Utf8PathBuf {
        self.results.join(format!("{target}-{key}.tar.zst"))
    }

    #[must_use]
    fn meta_path(&self, target: &Stage, key: &str) -> Utf8PathBuf {
        self.results.join(format!("{target}-{key}.meta.toml"))
    }

    /// Records what a result cost, beside the result itself.
    pub(crate) fn write_meta(&self, target: &Stage, key: &str, meta: &Meta) {
        let path = self.meta_path(target, key);
        let data = match toml::to_string_pretty(meta) {
            Ok(data) => data,
            Err(e) => return warn!("Failed serializing meta of {path}: {e}"),
        };
        // A result without its meta only costs us a report line, so this never fails a build.
        if let Err(e) = sys_fs().write_atomic(&path, &data) {
            warn!("Failed writing meta {path}: {e}");
        }
    }

    #[must_use]
    pub(crate) fn read_meta(&self, target: &Stage, key: &str) -> Option<Meta> {
        let path = self.meta_path(target, key);
        let data = sys_fs().read_to_string(&path).ok()?;
        toml::from_str(&data).inspect_err(|e| warn!("Dropping corrupted meta {path}: {e}")).ok()
    }

    pub(crate) async fn new_result(
        &self,
        target: &Stage,
        key: &str,
    ) -> Result<Option<ResultWriter>> {
        let dst = self.result_from_stage(target, key);
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
        let header = header_for("result.tar", built.len())?;
        self.w
            .append(&header, built)
            .await
            .map_err(|e| anyhow!("Failed appending tar to result: {e}"))
    }

    /// Seals the result, returning what it takes up on disk (0 when another build won).
    pub(crate) async fn finalize(self, md_ser: &str) -> Result<u64> {
        let Self { tmp, dst, mut w } = self;

        let header = header_for("md.toml", md_ser.len())?;
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
        let stored = zst.len() as u64;
        fs::write(&tmp, zst).await.map_err(|e| anyhow!("Failed writing result {tmp}: {e}"))?;
        info!("moving result to {dst}");
        fs::rename(&tmp, &dst).await.map_err(|e| anyhow!("Failed `mv {tmp} {dst}`: {e}"))?;
        Ok(stored)
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

pub(crate) async fn extract_just(src: &Utf8Path, fname: &str) -> Result<Vec<u8>> {
    let zst = fs::read(&src).await.map_err(|e| anyhow!("Failed reading tarball {src}: {e}"))?;
    let tar = decompress(src, &zst)?;

    let mut inner = Vec::new();
    let mut ar = TarArchive::new(tar.as_slice());
    let mut entries = ar.entries().map_err(|e| anyhow!("Failed reading {src}: {e}"))?;
    while let Some(entry) = entries.next().await {
        let mut f = entry.map_err(|e| anyhow!("Failed streaming tarball {src}: {e}"))?;
        let name = f
            .path()
            .map_err(|e| anyhow!("Failed decoding {src} entry name: {e}"))?
            .to_string_lossy()
            .to_string();
        if name == fname {
            let _ = f
                .read_to_end(&mut inner)
                .await
                .map_err(|e| anyhow!("Failed extracting {fname} from {src}: {e}"))?;
            break;
        }
    }
    Ok(inner)
}

fn header_for(fname: &str, len: usize) -> Result<Header> {
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
