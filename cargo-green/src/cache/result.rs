//! Build results are the artifacts of the runner's `rustc` (and build scripts)
//! invocations bundled as a tarball.

use std::{env, time::Duration};

use anyhow::{Result, anyhow, bail};
use async_compression::tokio::{bufread::GzipDecoder, write::GzipEncoder};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info, warn};
use reqwest::{
    Body, Client, StatusCode,
    header::{CONTENT_LENGTH, CONTENT_TYPE},
};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter},
};
use tokio_stream::StreamExt;
use tokio_tar::{Archive as TarArchive, Builder as TarBuilder, EntryType, Header};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::{build::SOURCE_DATE_EPOCH, dirs::Dirs, stage::Stage};

/// R2 custom domain serving published build-result tarballs (`{stage}.tar.gz`).
/// Override with `$CARGOGREEN_RESULTS_BASE_URL`; set it empty to disable remote fetching.
pub(crate) const RESULTS_BASE_URL: &str = "https://results.cargo.green";

/// Whether cargo is operating offline (`--offline`/`--frozen` surface as this env).
/// When set, all remote results traffic (fetch + publish) is skipped.
fn offline() -> bool {
    matches!(env::var("CARGO_NET_OFFLINE").as_deref(), Ok("1" | "true"))
}

impl Dirs {
    pub(crate) fn result_from_stage(&self, target: &Stage) -> Utf8PathBuf {
        self.results.join(format!("{target}.tar.gz"))
    }

    /// On a local-disk miss, try fetching `{target}.tar.gz` from the remote results store
    /// (R2 custom domain) into `dst`. Returns whether the result is now present locally.
    ///
    /// Any network/HTTP failure (offline, 404, 5xx, …) is treated as a cache miss
    /// (`Ok(false)`) so the caller falls back to building. Only local filesystem
    /// errors bubble up.
    pub(crate) async fn fetch_remote_result(&self, target: &Stage, dst: &Utf8Path) -> Result<bool> {
        if offline() {
            debug!("offline: skipping remote result fetch for {target}");
            return Ok(false);
        }
        let base = match env::var("CARGOGREEN_RESULTS_BASE_URL") {
            Ok(base) if base.is_empty() => return Ok(false), // remote results explicitly disabled
            Ok(base) => base,
            Err(_) => RESULTS_BASE_URL.to_owned(),
        };
        let url = format!("{base}/{target}.tar.gz");

        let client = Client::builder()
            .connect_timeout(Duration::from_secs(4))
            .build()
            .map_err(|e| anyhow!("HTTP client's config/TLS failed: {e}"))?;

        info!("GETing {url}");
        let resp = match client.get(&url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                debug!("remote results unreachable ({url}): {e}");
                return Ok(false);
            }
        };
        if resp.status() == StatusCode::NOT_FOUND {
            debug!("no remote result for {target} at {url}");
            return Ok(false);
        }
        let resp = match resp.error_for_status() {
            Ok(resp) => resp,
            Err(e) => {
                debug!("remote results error for {url}: {e}");
                return Ok(false);
            }
        };

        // Stream body to a temp file on the same partition, then atomically move into place.
        let tmp = self.tmp.join(format!("{}.tar.gz", Uuid::new_v4()));
        let mut f = File::create(&tmp)
            .await
            .map_err(|e| anyhow!("Failed opening (W) downloaded result {tmp}: {e}"))?;
        let mut written: u64 = 0;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(e) => {
                    debug!("Failed downloading {url}: {e}");
                    let _ = fs::remove_file(&tmp).await;
                    return Ok(false);
                }
            };
            f.write_all(&chunk).await.map_err(|e| anyhow!("Failed writing {tmp}: {e}"))?;
            written += chunk.len() as u64;
        }
        f.flush().await.map_err(|e| anyhow!("Failed flushing {tmp}: {e}"))?;
        info!("fetched {written} bytes from {url}");

        // NOTE: TOCTOU on dst is okay as long as `mv` is atomic
        if dst.exists() {
            fs::remove_file(&tmp).await.map_err(|e| anyhow!("Failed `rm {tmp}`: {e}"))?;
        } else {
            info!("moving downloaded result to {dst}");
            fs::rename(&tmp, dst).await.map_err(|e| anyhow!("Failed `mv {tmp} {dst}`: {e}"))?;
        }
        Ok(true)
    }

    /// Publish a freshly written result tarball (`src`) to the remote results store via PUT,
    /// streaming the file body. No-op unless `$CARGOGREEN_RESULTS_UPLOAD_URL` is set (writes
    /// need credentials, unlike public reads). `$CARGOGREEN_RESULTS_TOKEN`, if set, is sent as
    /// a bearer token. Best-effort: network/HTTP failures only `warn!` so builds never break.
    pub(crate) async fn publish_remote_result(&self, target: &Stage, src: &Utf8Path) -> Result<()> {
        if offline() {
            debug!("offline: skipping remote result publish for {target}");
            return Ok(());
        }
        let base = match env::var("CARGOGREEN_RESULTS_UPLOAD_URL") {
            Ok(base) if !base.is_empty() => base,
            _ => return Ok(()), // publishing disabled (default)
        };
        let url = format!("{base}/{target}.tar.gz");

        let file = File::open(src)
            .await
            .map_err(|e| anyhow!("Failed opening (RO) {src} to publish: {e}"))?;
        let len = file.metadata().await.map(|m| m.len()).ok();
        let body = Body::wrap_stream(ReaderStream::new(file));

        let client = Client::builder()
            .connect_timeout(Duration::from_secs(4))
            .build()
            .map_err(|e| anyhow!("HTTP client's config/TLS failed: {e}"))?;

        let mut req = client.put(&url).header(CONTENT_TYPE, "application/gzip");
        if let Some(len) = len {
            req = req.header(CONTENT_LENGTH, len);
        }
        if let Ok(token) = env::var("CARGOGREEN_RESULTS_TOKEN")
            && !token.is_empty()
        {
            req = req.bearer_auth(token);
        }

        info!("PUTing {url}");
        match req.body(body).send().await {
            Ok(resp) => match resp.error_for_status() {
                Ok(_) => info!("published result {target} to {url}"),
                Err(e) => warn!("failed publishing result to {url}: {e}"),
            },
            Err(e) => warn!("failed reaching {url} to publish result: {e}"),
        }
        Ok(())
    }

    pub(crate) async fn new_result(&self, target: &Stage) -> Result<Option<ResultWriter>> {
        let dst = self.result_from_stage(target);
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
        let encoder = GzipEncoder::new(writer); // FIXME: replace with pure-Rust zstd eg. libzstd-rs-sys
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

        if dst.exists() {
            fs::remove_file(&tmp).await.map_err(|e| anyhow!("Failed `rm {tmp}`: {e}"))?;
        } else {
            info!("moving result to {dst}");
            fs::rename(&tmp, &dst).await.map_err(|e| anyhow!("Failed `mv {tmp} {dst}`: {e}"))?;
        }
        Ok(())
    }
}

pub(crate) async fn extract_just(src: &Utf8Path, fname: &str) -> Result<Vec<u8>> {
    let mut gz = Vec::new();
    let mut f =
        File::open(&src).await.map_err(|e| anyhow!("Failed opening (RO) tarball {src}: {e}"))?;
    let _ =
        f.read_to_end(&mut gz).await.map_err(|e| anyhow!("Failed reading tarball {src}: {e}"))?;

    let mut inner = Vec::new();
    let mut ar = TarArchive::new(GzipDecoder::new(BufReader::new(gz.as_slice())));
    let mut entries = ar.entries().map_err(|e| anyhow!("Failed reading {src}: {e}"))?;
    while let Some(Ok(mut f)) = entries.next().await {
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
