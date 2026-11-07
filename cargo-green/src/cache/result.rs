//! Build results are the artifacts of the runner's `rustc` (and build scripts) invocations:
//! the tarball BuildKit's tar exporter produced, with our `md.toml` appended to it.

use std::io::Read;

use anyhow::{Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info};
use ruzstd::{
    decoding::StreamingDecoder,
    encoding::{CompressionLevel, compress_to_vec},
};
use tokio_stream::StreamExt;
use tokio_tar::{Archive as TarArchive, EntryType, Header};

use crate::{
    build::{SOURCE_DATE_EPOCH, STDERR, STDOUT},
    dirs::Dirs,
    stage::Stage,
    sys::fs,
};

/// Results' own entry, appended last.
///
/// Can't clash with what the tar exporter produces: the stdio files and the out dir.
pub(crate) const MD_TOML: &str = "md.toml";

/// Tar archives are made of 512-byte blocks.
const BLOCK: usize = 512;

impl Dirs {
    pub(crate) fn result_from_stage(&self, target: &Stage) -> Utf8PathBuf {
        self.results.join(format!("{target}.tar.zst"))
    }

    /// Takes the tar exporter's tarball, to append to then store.
    pub(crate) async fn new_result(
        &self,
        target: &Stage,
        mut tar: Vec<u8>,
    ) -> Result<Option<ResultWriter>> {
        let dst = self.result_from_stage(target);
        if fs().exists(&dst) {
            return Ok(None);
        }
        // NOTE: TOCTOU on dst is okay as long as writing it is atomic
        tar.truncate(entries_end(&tar).await?);
        Ok(Some(ResultWriter { dst, tar }))
    }
}

/// Where a tarball's entries end
/// ie. where its end-of-archive marker (two zeroed blocks, plus any padding) starts:
/// where more entries go
async fn entries_end(tar: &[u8]) -> Result<usize> {
    let mut end = 0;
    let mut ar = TarArchive::new(tar);
    let mut entries = ar.entries().map_err(|e| anyhow!("Failed reading tarball: {e}"))?;
    while let Some(entry) = entries.next().await {
        let f = entry.map_err(|e| anyhow!("Failed streaming tarball: {e}"))?;
        // NOTE: an entry's PAX or GNU long name headers come before its own: they're counted in
        let size: usize = f.effective_size().try_into()?;
        end = usize::try_from(f.raw_file_position())? + size.next_multiple_of(BLOCK);
    }
    if end == 0 || tar.len() < end + 2 * BLOCK {
        bail!("Truncated tarball: {}B, entries end at {end}B", tar.len())
    }
    Ok(end)
}

/// The tar exporter's tarball (cut at its end-of-archive marker), held in memory then completed
/// with our entry and compressed in one go.
///
/// Results are bit-reproducible, as is the tarball they start from,
/// given Md's contents are:
/// * our entry's header is all fixed (see [`header_for`]): GNU format, path, size, mode,
///   uid & gid, empty user & group names, mtime `$SOURCE_DATE_EPOCH`, no PAX records
/// * `md.toml` serializes an Md made of items in the order they were pushed in
/// * padding and the end-of-archive marker are zeros
/// * compression is ruzstd's, at a fixed level, without checksum nor dictionary:
///   a zstd frame holds neither names nor timestamps
pub(crate) struct ResultWriter {
    tar: Vec<u8>,
    dst: Utf8PathBuf,
}

impl ResultWriter {
    /// No-op when another build already stored the result
    pub(crate) fn finalize(self, md_ser: &str) -> Result<()> {
        let Self { dst, mut tar } = self;

        macro_rules! check {
            () => {{
                if fs().exists(&dst) {
                    debug!("{dst} already exists, dropping work");
                    return Ok(());
                }
            }};
        }

        check!();
        let header = header_for(MD_TOML, md_ser.len())?;
        tar.extend_from_slice(header.as_bytes());
        tar.extend_from_slice(md_ser.as_bytes());
        // Pad our entry, then end the archive with two zeroed blocks
        tar.resize(tar.len().next_multiple_of(BLOCK) + 2 * BLOCK, 0);

        check!();
        info!("compressing result");
        let zst = compress(&tar);
        check!();
        info!("writing result to {dst}");
        fs().write_atomic_bytes(&dst, &zst, 0o644)
            .map_err(|e| anyhow!("Failed writing result {dst}: {e}"))?;
        Ok(())
    }
}

// TODO: checkup on (Implement the Default and Better compression levels)[https://github.com/KillingSpark/zstd-rs/pull/135]
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

/// Reads a result: the tar exporter's tarball, with [`MD_TOML`] last.
fn read(src: &Utf8Path) -> Result<Vec<u8>> {
    let zst = fs().read(src).map_err(|e| anyhow!("Failed reading tarball {src}: {e}"))?;
    decompress(src, &zst)
}

/// Reads a result, given it looks like one of ours: anything may be found in results' dir.
///
/// Checks what extracting it relies on: entries that parse and that hold no surprises (see
/// [`unexpected_header_field`], else extracting panics), the stdio files every RUN writes, and
/// our entry last.
pub(crate) async fn read_sound(src: &Utf8Path) -> Result<Vec<u8>> {
    let tar = read(src)?;
    let _ = entries_end(&tar).await?;

    let (mut last, mut has_stdout, mut has_stderr) = (String::new(), false, false);
    let mut ar = TarArchive::new(tar.as_slice());
    let mut entries = ar.entries().map_err(|e| anyhow!("Failed reading tarball: {e}"))?;
    while let Some(entry) = entries.next().await {
        let f = entry.map_err(|e| anyhow!("Failed streaming tarball: {e}"))?;
        last = f.path().map_err(|e| anyhow!("Bad entry name: {e}"))?.to_string_lossy().into_owned();
        if let Some(field) = unexpected_header_field(f.header()) {
            bail!("Unexpected {field} in the header of {last}")
        }
        has_stdout |= last == STDOUT;
        has_stderr |= last == STDERR;
    }
    if !has_stdout || !has_stderr {
        bail!("Missing stdio entries")
    }
    if last != MD_TOML {
        bail!("Last entry isn't {MD_TOML}: {last:?}")
    }
    Ok(tar)
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
    if let Some(field) = unexpected_header_field(header) {
        panic!("Unexpected {field} in tar header: {header:?}")
    }
}

/// Which field of this header isn't what the tar exporter (given our global `ARG`) and we write
#[must_use]
fn unexpected_header_field(header: &Header) -> Option<&'static str> {
    [
        ("uid", header.uid().ok() == Some(0)),
        ("gid", header.gid().ok() == Some(0)),
        ("mtime", header.mtime().ok() == Some(SOURCE_DATE_EPOCH)),
        ("username", header.username() == Ok(Some(""))),
        ("groupname", header.groupname() == Ok(Some(""))),
        ("device major", header.device_major().ok() == Some(Some(0))),
        ("device minor", header.device_minor().ok() == Some(Some(0))),
    ]
    .into_iter()
    .find_map(|(field, expected)| (!expected).then_some(field))
}

#[cfg(test)]
mod are_ya_winning {
    use anyhow::{Result, anyhow};
    use camino::{Utf8Path, Utf8PathBuf};
    use ruzstd::encoding::{CompressionLevel, compress_to_vec};
    use tokio::io::AsyncReadExt;
    use tokio_stream::StreamExt;
    use tokio_tar::{Archive as TarArchive, Builder as TarBuilder};

    use super::{
        BLOCK, MD_TOML, ResultWriter, assert_tarball_header, compress, decompress, entries_end,
        header_for, read, read_sound,
    };
    use crate::{
        dirs::Dirs,
        stage::Stage,
        sys::{Sys, fake::FakeFs, fs},
    };

    /// Extracts the named entries of a result in one pass, in the order asked, each empty when absent.
    async fn extract<const N: usize>(src: &Utf8Path, fnames: [&str; N]) -> Result<[Vec<u8>; N]> {
        let tar = read(src)?;

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
    async fn writing_results_is_bit_reproducible() -> Result<()> {
        let fakefs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fakefs.clone(), ..Sys::fake() });

        let mut exported = TarBuilder::new(vec![]);
        for (fname, data) in [("1", "out"), ("deps/libfoo-3333333333333333.rmeta", "rmeta")] {
            exported.append(&header_for(fname, data.len())?, data.as_bytes()).await?;
        }
        exported.finish().await?;
        let exported = exported.into_inner().await?;

        let mut written = vec![];
        for dst in ["/a/out-3333333333333333.tar.zst", "/b/out-3333333333333333.tar.zst"] {
            let w = ResultWriter {
                dst: dst.into(),
                tar: exported[..entries_end(&exported).await?].to_vec(),
            };
            w.finalize("stamp = 3\n")?;
            written.push(fs().read(dst.into())?);
        }
        assert_eq!(written[0], written[1]);

        // Each header, ours included, is as results' must be
        let tar = read("/a/out-3333333333333333.tar.zst".into())?;
        let mut ar = TarArchive::new(tar.as_slice());
        let mut entries = ar.entries()?;
        let mut names = vec![];
        while let Some(entry) = entries.next().await {
            let f = entry?;
            assert_tarball_header(f.header());
            assert!(f.header().as_gnu().is_some() || f.header().as_ustar().is_some());
            names.push(f.path()?.to_string_lossy().into_owned());
        }
        assert_eq!(names, ["1", "deps/libfoo-3333333333333333.rmeta", MD_TOML]);
        Ok(())
    }

    #[tokio::test]
    async fn only_sound_results_are_read() -> Result<()> {
        async fn tarball(entries: &[(&str, &str)]) -> Result<Vec<u8>> {
            let mut w = TarBuilder::new(vec![]);
            for (fname, data) in entries {
                w.append(&header_for(fname, data.len())?, data.as_bytes()).await?;
            }
            w.finish().await?;
            Ok(w.into_inner().await?)
        }

        let fakefs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fakefs.clone(), ..Sys::fake() });
        let src = Utf8Path::new("/results/out-3333333333333333.tar.zst");
        let zst = |tar: &[u8]| compress_to_vec(tar, CompressionLevel::Fastest);

        let good = [("1", "out"), ("2", "err"), ("deps/a.rmeta", "rmeta"), (MD_TOML, "stamp = 3")];
        fs().write_atomic_bytes(src, &zst(&tarball(&good).await?), 0o644)?;
        assert!(read_sound(src).await.is_ok());

        for (bad, why) in [
            (b"nonsense".to_vec(), "Failed"),
            (zst(b"nonsense"), "Failed streaming"),
            // The earlier versions wrote the exporter's tarball as an entry
            (zst(&tarball(&[("result.tar", "tar"), (MD_TOML, "")]).await?), "Missing stdio"),
            (zst(&tarball(&good[..3]).await?), "Last entry"),
        ] {
            fs().write_atomic_bytes(src, &bad, 0o644)?;
            let e = read_sound(src).await.expect_err(why).to_string();
            assert!(e.contains(why), "{why:?} not in {e:?}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn extracting_entries_in_one_pass() -> Result<()> {
        let fakefs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fakefs.clone(), ..Sys::fake() });
        let dst: Utf8PathBuf = "/results/out-3333333333333333.tar.zst".into();

        // As the tar exporter writes them: entries, then two zeroed blocks (no further padding)
        let mut exported = TarBuilder::new(vec![]);
        for (fname, data) in [("1", "out"), ("deps/libfoo-3333333333333333.rmeta", "rmeta")] {
            exported.append(&header_for(fname, data.len())?, data.as_bytes()).await?;
        }
        exported.finish().await?;
        let exported = exported.into_inner().await?;
        assert_eq!(entries_end(&exported).await?, exported.len() - 2 * BLOCK);

        let dirs = Dirs { results: "/results".into(), buildkit: "/b".into(), var_run: "/r".into() };
        let target = Stage::new("out-3333333333333333")?;
        let w = dirs.new_result(&target, exported.clone()).await?.expect("not stored yet");
        w.finalize("stamp = 3")?;
        assert_eq!(fakefs.written(), std::slice::from_ref(&dst));

        assert!(dirs.new_result(&target, exported.clone()).await?.is_none(), "already stored");
        let w = ResultWriter { dst: dst.clone(), tar: vec![] };
        w.finalize("not stored")?;
        assert_eq!(fakefs.written(), std::slice::from_ref(&dst), "another build already stored it");

        let [md, missing, out] = extract(&dst, [MD_TOML, "nope", "1"]).await?;
        assert_eq!(out, b"out");
        assert_eq!(md, b"stamp = 3");
        assert!(missing.is_empty());

        // The exporter's entries come first, untouched: only its end-of-archive marker moved
        let tar = read(&dst)?;
        assert_eq!(tar[..exported.len() - 2 * BLOCK], exported[..exported.len() - 2 * BLOCK]);
        assert_eq!(entries_end(&tar).await?, tar.len() - 2 * BLOCK);
        Ok(())
    }
}
