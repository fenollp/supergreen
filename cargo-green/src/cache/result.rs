//! Build results are the artifacts of the runner's `rustc` (and build scripts)
//! invocations bundled as a tarball.

use std::{collections::BTreeMap, env};

use anyhow::{Result, anyhow, bail};
use async_compression::tokio::{bufread::GzipDecoder, write::GzipEncoder};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info, warn};
use reqwest::{Body, Client, StatusCode};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter},
};
use tokio_stream::StreamExt;
use tokio_tar::{Archive as TarArchive, Builder as TarBuilder, EntryType, Header};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::{
    build::SOURCE_DATE_EPOCH,
    cache::s3::{S3Config, put_object},
    dirs::Dirs,
    stage::Stage,
};

/// R2 custom domain serving published build-result tarballs (`{stage}.tar.gz`), read-only and
/// public (free egress via the CDN). Override with `$CARGOGREEN_RESULTS_BASE_URL`; set it empty
/// to disable remote fetching.
pub(crate) const RESULTS_BASE_URL: &str = "https://results.cargo.green";

/// Whether cargo is operating offline (`--offline`/`--frozen` surface as this env).
/// When set, all remote results traffic (fetch + publish) is skipped.
fn offline() -> bool {
    matches!(env::var("CARGO_NET_OFFLINE").as_deref(), Ok("1" | "true"))
}

/// Connect timeout for all remote results traffic.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

fn http_client() -> Result<Client> {
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|e| anyhow!("HTTP client's config/TLS failed: {e}"))
}

impl Dirs {
    pub(crate) fn result_from_stage(&self, target: &Stage) -> Utf8PathBuf {
        self.results.join(format!("{target}.tar.gz"))
    }

    /// On a local-disk miss, try fetching `{target}.tar.gz` from the public results domain
    /// (R2 custom domain) into `dst`. Returns whether the result is now present locally.
    ///
    /// The body is streamed to a temp file, then verified against the sidecar `{…}.sha256`
    /// object (when present) before being atomically moved into place. Any network/HTTP
    /// failure (offline, 404, 5xx, checksum mismatch, …) is treated as a cache miss
    /// (`Ok(false)`) so the caller falls back to building. Only local filesystem errors bubble.
    pub(crate) async fn fetch_remote_result(&self, target: &Stage, dst: &Utf8Path) -> Result<bool> {
        if offline() {
            debug!("offline: skipping remote result fetch for {target}");
            return Ok(false);
        }
        let base = match env::var(ENV_RESULTS_BASE_URL!()) {
            Ok(base) if base.is_empty() => return Ok(false), // remote results explicitly disabled
            Ok(base) => base,
            Err(_) => RESULTS_BASE_URL.to_owned(),
        };
        let url = format!("{base}/{target}.tar.gz");

        let client = http_client()?;

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

        // Stream body to a temp file on the same partition.
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

        // Integrity check against the published sidecar checksum, when available.
        if !verify_sidecar_sha256(&client, &base, target, &tmp).await? {
            let _ = fs::remove_file(&tmp).await;
            return Ok(false);
        }

        // NOTE: TOCTOU on dst is okay as long as `mv` is atomic
        if dst.exists() {
            fs::remove_file(&tmp).await.map_err(|e| anyhow!("Failed `rm {tmp}`: {e}"))?;
        } else {
            info!("moving downloaded result to {dst}");
            fs::rename(&tmp, dst).await.map_err(|e| anyhow!("Failed `mv {tmp} {dst}`: {e}"))?;
        }
        Ok(true)
    }

    /// Publish a freshly written result tarball (`src`) to the R2 S3 endpoint via a SigV4-signed
    /// `PUT`, streaming the file body, alongside a tiny `{…}.sha256` sidecar used by readers to
    /// verify integrity. No-op unless the `$CARGOGREEN_RESULTS_S3_*` credentials are configured.
    /// Best-effort: network/HTTP failures only `warn!` so builds never break.
    pub(crate) async fn publish_remote_result(&self, target: &Stage, src: &Utf8Path) -> Result<()> {
        if offline() {
            debug!("offline: skipping remote result publish for {target}");
            return Ok(());
        }
        let Some(cfg) = S3Config::from_env() else { return Ok(()) };

        if let Err(e) = self.publish_inner(&cfg, target, src).await {
            warn!("failed publishing result {target}: {e}");
        }
        Ok(())
    }

    async fn publish_inner(&self, cfg: &S3Config, target: &Stage, src: &Utf8Path) -> Result<()> {
        let client = http_client()?;

        // SigV4 requires the exact payload hash; since we stream the file unchanged, the file's
        // SHA-256 *is* the payload hash. Reuse it as both the signed hash and the sidecar value.
        let sha = sha256::try_async_digest(src.as_std_path())
            .await
            .map_err(|e| anyhow!("Failed hashing {src}: {e}"))?;
        let len = fs::metadata(src).await.map_err(|e| anyhow!("Failed `stat {src}`: {e}"))?.len();

        let file = File::open(src)
            .await
            .map_err(|e| anyhow!("Failed opening (RO) {src} to publish: {e}"))?;
        let body = Body::wrap_stream(ReaderStream::new(file));

        let mut meta = BTreeMap::new();
        meta.insert("x-amz-meta-sha256".to_owned(), sha.clone());

        let key = format!("{target}.tar.gz");
        info!("PUTing s3://…/{key} ({len}B)");
        put_object(cfg, &client, &key, body, len, &sha, "application/gzip", &meta).await?;

        // Sidecar: hex SHA-256, fetched by readers over the public domain to verify downloads.
        let sidecar = format!("{key}.sha256");
        let payload = sha256::digest(sha.as_bytes());
        put_object(
            cfg,
            &client,
            &sidecar,
            Body::from(sha.clone()),
            sha.len() as u64,
            &payload,
            "text/plain",
            &BTreeMap::new(),
        )
        .await?;

        info!("published result {target} ({sha})");
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

/// Verify a downloaded tarball at `tmp` against its published `{target}.tar.gz.sha256` sidecar.
///
/// Returns `Ok(true)` to accept (checksum matched, or no sidecar/network to verify against) and
/// `Ok(false)` to reject (sidecar present but mismatched — corruption/truncation/tampering).
async fn verify_sidecar_sha256(
    client: &Client,
    base: &str,
    target: &Stage,
    tmp: &Utf8Path,
) -> Result<bool> {
    let url = format!("{base}/{target}.tar.gz.sha256");
    let resp = match client.get(&url).send().await {
        Ok(resp) => resp,
        Err(e) => {
            debug!("no checksum to verify against ({url}): {e}");
            return Ok(true);
        }
    };
    if resp.status() == StatusCode::NOT_FOUND {
        debug!("no sidecar checksum for {target}; accepting unverified");
        return Ok(true);
    }
    let expected = match resp.error_for_status() {
        Ok(resp) => match resp.text().await {
            Ok(text) => text.trim().to_owned(),
            Err(e) => {
                debug!("failed reading checksum {url}: {e}");
                return Ok(true);
            }
        },
        Err(e) => {
            debug!("checksum unavailable {url}: {e}");
            return Ok(true);
        }
    };

    let actual = sha256::try_async_digest(tmp.as_std_path())
        .await
        .map_err(|e| anyhow!("Failed hashing {tmp}: {e}"))?;
    if actual == expected {
        debug!("checksum OK for {target}: {actual}");
        Ok(true)
    } else {
        warn!("checksum mismatch for {target}: expected {expected}, got {actual} — discarding");
        Ok(false)
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
