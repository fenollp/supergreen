//! Build results are the artifacts of the runner's `rustc` (and build scripts)
//! invocations bundled as a tarball.

use anyhow::{anyhow, bail, Result};
use async_compression::tokio::write::GzipEncoder;
use camino::Utf8PathBuf;
use log::{debug, info};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncWriteExt, BufWriter},
};
use tokio_tar::{Builder as TarBuilder, EntryType, Header};
use uuid::Uuid;

use crate::{build::SOURCE_DATE_EPOCH, dirs::Dirs, stage::Stage};

impl Dirs {
    pub(crate) async fn new_result(&self, target: &Stage) -> Result<Option<ResultWriter>> {
        let dst = self.results.join(format!("{target}.tar.gz"));
        let tmp = self.tmp.join(format!("{}.tar.gz", Uuid::new_v4()));
        if dst.exists() {
            return Ok(None);
        }
        debug!("writing result to {tmp}");

        // NOTE: TOCTOU on dst is okay as long as `mv` is atomic

        let mut opts = OpenOptions::new();
        opts.create(true).write(true).truncate(true);
        let f =
            opts.open(&tmp).await.map_err(|e| anyhow!("Failed opening (W) result {tmp}: {e}"))?;

        let writer = BufWriter::new(f);
        let encoder = GzipEncoder::new(writer);
        let w = TarBuilder::new(encoder);
        Ok(Some(ResultWriter { tmp, dst, w }))
    }
}

/// An async buffered archive writer
pub(crate) struct ResultWriter {
    w: TarBuilder<GzipEncoder<BufWriter<File>>>,
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

    pub(crate) async fn finalize(self, md_ser: String) -> Result<()> {
        let Self { tmp, dst, mut w } = self;

        let header = header_for("md.toml", md_ser.len())?;
        w.append(&header, md_ser.as_bytes())
            .await
            .map_err(|e| anyhow!("Failed appending Md to result: {e}"))?;

        let mut finished_encoder =
            w.into_inner().await.map_err(|e| anyhow!("Failed finishing result: {e}"))?;
        finished_encoder.shutdown().await.map_err(|e| anyhow!("Failed flushing result: {e}"))?;

        if !dst.exists() {
            info!("moving result to {dst}");
            fs::rename(&tmp, &dst).await.map_err(|e| anyhow!("Failed `mv {tmp} {dst}`: {e}"))?;
        }
        Ok(())
    }
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
    use async_compression::tokio::bufread::GzipDecoder;
    use tokio::io::AsyncReadExt;
    use tokio_stream::StreamExt;
    use tokio_tar::Archive as TarArchive;

    let buf = vec![];
    let writer = BufWriter::new(buf);
    let encoder = GzipEncoder::new(writer);
    let mut w = TarBuilder::new(encoder);

    let some_data = vec![10, 10, 10];
    let header = header_for("some.file", some_data.len())?;
    w.append(&header, some_data.as_slice()).await?;

    w.finish().await?;
    let mut final_buf_writer = w.into_inner().await?;
    final_buf_writer.flush().await?;
    let mut final_encoder = final_buf_writer.into_inner();
    final_encoder.shutdown().await?;

    let buf = final_encoder.into_inner();
    let decoder = GzipDecoder::new(&buf[..]);
    let mut r = TarArchive::new(decoder);

    let mut entries = r.entries()?;
    let entry = entries.next().await.expect("we wrote 1 entry");
    let mut entry = entry?;
    let header_bis = entry.header();
    assert_tarball_header(header_bis);

    assert_eq!(header.as_bytes(), header_bis.as_bytes());
    assert_eq!(header.size().ok(), header_bis.size().ok());
    assert_eq!(header.path().ok(), header_bis.path().ok());
    assert_eq!(header.link_name().ok(), header_bis.link_name().ok());

    let mut buf = Vec::new();
    entry.read_to_end(&mut buf).await?;
    assert_eq!(buf, some_data);

    Ok(())
}
