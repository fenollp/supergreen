//! Build results are the artifacts of the runner's `rustc` (and build scripts)
//! invocations bundled as a tarball.

use anyhow::{Result, anyhow, bail};
use async_compression::tokio::{bufread::GzipDecoder, write::GzipEncoder};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter},
};
use tokio_stream::StreamExt;
use tokio_tar::{Archive as TarArchive, Builder as TarBuilder, EntryType, Header};
use uuid::Uuid;

use crate::{
    build::SOURCE_DATE_EPOCH,
    dirs::Dirs,
    md::DIESES,
    stage::Stage,
    stats::{Miss, why_missed},
    sys::sys,
};

/// What a stored result knows about itself, kept beside the tarball.
///
/// The recipe is here (and not only inside the tarball) so that explaining a cache miss
/// costs a small read rather than decompressing an artifact archive; and so a miss can be
/// explained against results whose tarball we would never have opened.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Meta {
    /// How long the runner took to produce this, in milliseconds.
    pub(crate) took_ms: u64,

    /// The recipe this came out of, as hashed into the key.
    pub(crate) recipe: String,
}

/// Names the exact build a result came out of.
///
/// `cargo`'s metadata hash (our [`crate::md::MdId`]) tells stages apart but says nothing of
/// what goes into one: it hashes the package, its features, profile and target, plus the same
/// of its dependencies — not the environment we forward, not the base image, not the toolchain
/// that lives in it, not our own recipe-writing. Two builds that disagree on any of those share
/// an `MdId`, so keying results on it alone hands the first build's artifacts to the second.
///
/// The Containerfile is the whole input, spelled out: it carries the base image, every
/// dependency's stage, the `env` block and the `rustc` call, and names the version of us that
/// wrote it. Host paths are rewritten out of it, so the same build hashes the same anywhere.
#[must_use]
pub(crate) fn result_key(recipe: &str) -> String {
    sha256::digest(recipe)[..16].to_owned()
}

/// The recipe a Containerfile stands for: only the lines the runner is handed.
///
/// Our own `##` annotations never reach it (see `send_containerfile`) and vary with settings
/// that change nothing about the build, so they tell no two builds apart, and a cache miss is
/// never explained by one of them.
#[must_use]
pub(crate) fn recipe_of(containerfile: &str) -> String {
    containerfile.lines().filter(|line| !line.starts_with(DIESES)).collect::<Vec<_>>().join("\n")
}

#[test]
fn a_result_is_named_after_the_very_build_it_came_out_of() {
    let recipe = "FROM rust AS rust-base
FROM rust-base AS dep-mycrate
RUN \
    env CARGO_PKG_NAME=mycrate \
      rustc --crate-name mycrate src/lib.rs
";
    let key = result_key(&recipe_of(recipe));
    assert_eq!(key.len(), 16, "{key}");
    assert_eq!(key, result_key(&recipe_of(recipe)), "same recipe, same result");

    assert_eq!(
        key,
        result_key(&recipe_of(&format!(
            "## an annotation of ours, stripped before the runner sees it\n{recipe}"
        ))),
        "annotations are not part of the build"
    );

    // Each of these leaves `cargo`'s metadata hash (and so the stage name) untouched.
    for (what, recipe) in [
        ("a forwarded env var", recipe.replace("CARGO_PKG_NAME=mycrate", "CARGO_PKG_NAME=other")),
        ("the base image", recipe.replace("FROM rust AS", "FROM rust:1.42 AS")),
        ("the rustc call", recipe.replace("src/lib.rs", "src/main.rs")),
    ] {
        assert_ne!(key, result_key(&recipe_of(&recipe)), "{what} makes for another result");
    }
}

impl Dirs {
    pub(crate) fn result_from_stage(&self, target: &Stage, key: &str) -> Utf8PathBuf {
        self.results.join(format!("{target}-{key}.tar.gz"))
    }

    #[must_use]
    fn meta_path(&self, target: &Stage, key: &str) -> Utf8PathBuf {
        self.results.join(format!("{target}-{key}.meta.toml"))
    }

    /// Records what a result cost and what it came from, beside the result itself.
    pub(crate) fn write_meta(&self, target: &Stage, key: &str, meta: &Meta) {
        let path = self.meta_path(target, key);
        let data = match toml::to_string_pretty(meta) {
            Ok(data) => data,
            Err(e) => return warn!("Failed serializing meta of {path}: {e}"),
        };
        // A result without its meta only costs us a report line, so this never fails a build.
        if let Err(e) = sys().fs.write_atomic(&path, &data) {
            warn!("Failed writing meta {path}: {e}");
        }
    }

    #[must_use]
    pub(crate) fn read_meta(&self, target: &Stage, key: &str) -> Option<Meta> {
        let path = self.meta_path(target, key);
        let data = sys().fs.read_to_string(&path).ok()?;
        toml::from_str(&data).inspect_err(|e| warn!("Dropping corrupted meta {path}: {e}")).ok()
    }

    /// Explains why `recipe` found no result of its own to replay.
    ///
    /// Names the closest thing we do have — another build of the same crate, that cargo
    /// would have called by the very same name — and reads the first line where the two
    /// recipes part ways.
    #[must_use]
    pub(crate) fn why_missed(&self, target: &Stage, key: &str, recipe: &str) -> Option<Miss> {
        let ours = format!("{target}-{key}.meta.toml");
        let prefix = format!("{target}-");
        let mut newest: Option<(std::time::SystemTime, Utf8PathBuf)> = None;
        for entry in sys().fs.read_dir(&self.results).ok()? {
            if entry == ours || !entry.starts_with(&prefix) || !entry.ends_with(".meta.toml") {
                continue;
            }
            let path = self.results.join(entry);
            let Ok(at) = std::fs::metadata(&path).and_then(|md| md.modified()) else { continue };
            if newest.as_ref().is_none_or(|(newest, _)| *newest < at) {
                newest = Some((at, path));
            }
        }

        let (_, path) = newest?;
        let had: Meta = toml::from_str(&sys().fs.read_to_string(&path).ok()?).ok()?;
        Some(why_missed(&had.recipe, recipe))
    }

    pub(crate) async fn new_result(
        &self,
        target: &Stage,
        key: &str,
    ) -> Result<Option<ResultWriter>> {
        let dst = self.result_from_stage(target, key);
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

    /// Seals the result, returning what it takes up on disk (0 when another build won).
    pub(crate) async fn finalize(self, md_ser: &str) -> Result<u64> {
        let Self { tmp, dst, mut w } = self;

        let header = header_for("md.toml", md_ser.len())?;
        w.append(&header, md_ser.as_bytes())
            .await
            .map_err(|e| anyhow!("Failed appending Md to result: {e}"))?;

        let mut finished_encoder =
            w.into_inner().await.map_err(|e| anyhow!("Failed finishing result: {e}"))?;
        finished_encoder.shutdown().await.map_err(|e| anyhow!("Failed flushing result: {e}"))?;

        if dst.exists() {
            debug!("{dst} already exists, dropping work");
            fs::remove_file(&tmp).await.map_err(|e| anyhow!("Failed `rm {tmp}`: {e}"))?;
            return Ok(0);
        }
        info!("moving result to {dst}");
        fs::rename(&tmp, &dst).await.map_err(|e| anyhow!("Failed `mv {tmp} {dst}`: {e}"))?;
        Ok(fs::metadata(&dst).await.map(|md| md.len()).unwrap_or_default())
    }

    /// Drops a result that must not be reused (e.g. from a failed rustc call).
    pub(crate) async fn discard(self) {
        let Self { tmp, w, .. } = self;
        drop(w);
        if let Err(e) = fs::remove_file(&tmp).await {
            warn!("Failed discarding result {tmp}: {e}");
        }
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
