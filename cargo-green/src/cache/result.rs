//! Build results are the artifacts of the runner's `rustc` (and build scripts) invocations:
//! the tarball BuildKit's tar exporter produced, with our `md.toml` appended to it.

use std::{
    io::{self, Read, Seek, SeekFrom, Write},
    pin::Pin,
    process,
    task::{Context, Poll},
};

use anyhow::{Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info};
use ruzstd::{
    decoding::StreamingDecoder,
    encoding::{self, CompressionLevel},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_tar::{Archive as TarArchive, Entry as TarEntry, EntryType, Header};

use crate::{
    build::SOURCE_DATE_EPOCH,
    dirs::Dirs,
    stage::Stage,
    sys::{Spool, TAR_BLOCK, TAR_MODE, fs},
};

/// Results' own entry, appended last.
///
/// Can't clash with what the tar exporter produces: the stdio files and the out dir.
pub(crate) const MD_TOML: &str = "md.toml";

impl Dirs {
    pub(crate) fn result_from_stage(&self, target: &Stage) -> Utf8PathBuf {
        self.results.join(format!("{target}.tar.zst"))
    }

    /// Pipe tarball to a temporary file keyed by `pid` unless result tarball exists already
    pub(crate) fn new_spool(&self, target: &Stage) -> Result<Option<Spooled>> {
        let dst = self.result_from_stage(target);
        if fs().exists(&dst) {
            return Ok(None);
        }
        // NOTE: TOCTOU on dst is okay as long as writing it is atomic
        let path = self.results.join(format!(".{target}.{}.spool", process::id()));
        let spool = fs().spool(&path).map_err(|e| anyhow!("Failed creating spool {path}: {e}"))?;
        Ok(Some(Spooled { dst, spool: KeepsFirstError::new(spool) }))
    }
}

/// A tarball on its way to becoming the result at `dst`.
pub(crate) struct Spooled {
    dst: Utf8PathBuf,
    spool: KeepsFirstError<Box<dyn Spool>>,
}

/// Spools what's read through it, if it can: not being able to store a result doesn't fail a build.
pub(crate) struct Tee<R> {
    inner: R,
    spooled: Option<Spooled>,
    count: u64,
}

impl<R> Tee<R> {
    #[must_use]
    pub(crate) fn new(inner: R, spooled: Option<Spooled>) -> Self {
        Self { inner, spooled, count: 0 }
    }

    /// How many bytes went through
    #[must_use]
    pub(crate) fn count(&self) -> u64 {
        self.count
    }

    /// Checksum of what got spooled, in full. Takes as much memory.
    #[must_use]
    pub(crate) fn digest(&mut self) -> String {
        let Some(Spooled { spool, .. }) = &mut self.spooled else { return String::new() };
        let mut tar = vec![];
        let read = spool.inner.seek(SeekFrom::Start(0)).and(spool.inner.read_to_end(&mut tar));
        read.map(|_| sha256::digest(&tar)).unwrap_or_default()
    }

    /// Given where entries end: where the end-of-archive marker starts, and where ours goes.
    pub(crate) fn into_result(self, entries_end: u64) -> Result<Option<ResultWriter>> {
        let Some(Spooled { dst, spool }) = self.spooled else { return Ok(None) };
        let KeepsFirstError { inner: spool, error } = spool;
        if let Some(e) = error {
            bail!("Failed spooling {dst}: {e}")
        }
        if entries_end == 0 || self.count < entries_end + 2 * TAR_BLOCK {
            bail!("Truncated tarball: {}B, entries end at {entries_end}B", self.count)
        }
        Ok(Some(ResultWriter { dst, spool, entries_end }))
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for Tee<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        let before = buf.filled().len();
        let polled = Pin::new(&mut me.inner).poll_read(cx, buf);
        let read = &buf.filled()[before..];
        me.count += read.len() as u64;
        if let Some(Spooled { spool, .. }) = &mut me.spooled {
            let _ = spool.write_all(read); // Errors are kept
        }
        polled
    }
}

/// Where this entry ends: where the next one, or the end-of-archive marker, starts.
pub(crate) fn entry_end<R: AsyncRead + Unpin>(f: &TarEntry<TarArchive<R>>) -> u64 {
    // NOTE: an entry's PAX or GNU long name headers come before its own: they're counted in
    f.raw_file_position() + f.effective_size().next_multiple_of(TAR_BLOCK)
}

/// The tar exporter's tarball, spooled up to its end-of-archive marker, then completed with our
/// entry as it's compressed straight to disk.
///
/// Results are bit-reproducible, as is the tarball they start from
/// (see `doctor::tar_exporter_writes_deterministic_tarballs`)
/// given Md's contents are:
/// * our entry's header is all fixed (see [`header_for`]): GNU format, path, size, mode,
///   uid & gid, empty user & group names, mtime `$SOURCE_DATE_EPOCH`, no PAX records
/// * `md.toml` serializes an Md made of ordered items
/// * padding and the end-of-archive marker are zeros
/// * compression is ruzstd's, at a fixed level, without checksum nor dictionary:
///   a zstd frame holds neither names nor timestamps
pub(crate) struct ResultWriter {
    spool: Box<dyn Spool>,
    entries_end: u64,
    dst: Utf8PathBuf,
}

impl ResultWriter {
    /// No-op when another build already stored the result
    pub(crate) fn finalize(self, md_ser: &str) -> Result<()> {
        let Self { dst, mut spool, entries_end } = self;

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
        let mut ours = header.as_bytes().to_vec();
        ours.extend_from_slice(md_ser.as_bytes());
        // Pad our entry, then end the archive with two zeroed blocks
        let block = TAR_BLOCK as usize;
        ours.resize(ours.len().next_multiple_of(block) + 2 * block, 0);

        check!();
        info!("compressing result");
        if let Err(e) = spool.seek(SeekFrom::Start(0)) {
            bail!("Failed rewinding spool of result {dst}: {e}")
        }
        let mut zst = fs()
            .atomic_writer(&dst, TAR_MODE)
            .map_err(|e| anyhow!("Failed opening (Watomic) result {dst}: {e}"))?;
        if let Err(e) = compress(spool.take(entries_end).chain(ours.as_slice()), &mut zst) {
            bail!("Failed compressing result {dst}: {e}")
        }

        check!();
        info!("writing result to {dst}");
        if let Err(e) = zst.commit() {
            bail!("Failed writing result {dst}: {e}")
        }
        Ok(())
    }
}

// TODO: checkup on (Implement the Default and Better compression levels)[https://github.com/KillingSpark/zstd-rs/pull/135]
fn compress(tar: impl Read, into: impl Write) -> io::Result<()> {
    let (mut tar, mut into) = (KeepsFirstError::new(tar), KeepsFirstError::new(into));
    // The only level implemented besides `Uncompressed`: on results, ~3x faster than gzip's
    // default level, though ~50% bigger. No checksum (ruzstd's `hash` feature): less `unsafe`.
    encoding::compress(&mut tar, &mut into, CompressionLevel::Fastest);
    tar.error.or(into.error).map_or(Ok(()), Err)
}

/// ruzstd panics when it can't read or write, and not being able to store a result must not fail a
/// build: this never fails, and gets asked afterwards.
struct KeepsFirstError<T> {
    inner: T,
    error: Option<io::Error>,
}

impl<T> KeepsFirstError<T> {
    fn new(inner: T) -> Self {
        Self { inner, error: None }
    }
}

impl<R: Read> Read for KeepsFirstError<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.error.is_some() {
            return Ok(0);
        }
        Ok(self.inner.read(buf).unwrap_or_else(|e| {
            self.error = Some(e);
            0
        }))
    }
}

impl<W: Write> Write for KeepsFirstError<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.error.is_none()
            && let Err(e) = self.inner.write_all(buf)
        {
            self.error = Some(e);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Sync IO on async's side: it blocks, as all of what handles results does. Decompresses on the
/// go for TAR parsers, and writes what they extract.
pub(crate) struct Blocking<T>(pub(crate) T);

impl<R: Read + Unpin> AsyncRead for Blocking<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let n = self.get_mut().0.read(buf.initialize_unfilled())?;
        buf.advance(n);
        Poll::Ready(Ok(()))
    }
}

impl<W: Write + Unpin> AsyncWrite for Blocking<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(self.get_mut().0.write(buf))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(self.get_mut().0.flush())
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}

/// Reads a result as it decompresses: the tar exporter's tarball, with [`MD_TOML`] last.
pub(crate) fn stream(src: &Utf8Path) -> Result<impl AsyncRead + Unpin + Send + use<>> {
    let zst = fs().open(src).map_err(|e| anyhow!("Failed reading tarball {src}: {e}"))?;
    let tar = StreamingDecoder::new(zst).map_err(|e| anyhow!("Failed decoding {src}: {e}"))?;
    Ok(Blocking(tar))
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
    header.set_mode(TAR_MODE);
    header.set_cksum();
    assert_tarball_header(&header);
    Ok(header)
}

fn assert_tarball_header(header: &Header) {
    if let Some(field) = unexpected_header_field(header) {
        panic!("Unexpected {field} in tar header: {header:?}")
    }
}

/// Which field of this header isn't what the tar exporter (given our global `ARG`) and we write
#[must_use]
pub(crate) fn unexpected_header_field(header: &Header) -> Option<&'static str> {
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
    use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

    use anyhow::{Result, anyhow};
    use camino::{Utf8Path, Utf8PathBuf};
    use ruzstd::{
        decoding::StreamingDecoder,
        encoding::{CompressionLevel, compress_to_vec},
    };
    use tokio::io::{AsyncReadExt, copy, sink};
    use tokio_stream::StreamExt;
    use tokio_tar::{Archive as TarArchive, Builder as TarBuilder};

    use super::{
        KeepsFirstError, MD_TOML, ResultWriter, Spooled, Tee, assert_tarball_header, compress,
        header_for, stream,
    };
    use crate::{
        dirs::Dirs,
        stage::Stage,
        sys::{Sys, TAR_BLOCK, fake::FakeFs, fs},
    };

    const MARKER: usize = 2 * TAR_BLOCK as usize;

    /// As the tar exporter writes them: entries, then two zeroed blocks (no further padding)
    async fn exported() -> Result<Vec<u8>> {
        let mut exported = TarBuilder::new(vec![]);
        for (fname, data) in [("1", "out"), ("deps/libfoo-3333333333333333.rmeta", "rmeta")] {
            exported.append(&header_for(fname, data.len())?, data.as_bytes()).await?;
        }
        exported.finish().await?;
        Ok(exported.into_inner().await?)
    }

    fn spooled_at(dst: &str, exported: &[u8]) -> ResultWriter {
        ResultWriter {
            dst: dst.into(),
            spool: Box::new(Cursor::new(exported.to_vec())),
            entries_end: (exported.len() - MARKER) as u64,
        }
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
        decompress(src, &stored(src)?)
    }

    fn stored(src: &Utf8Path) -> Result<Vec<u8>> {
        let mut zst = vec![];
        let _ = fs().open(src)?.read_to_end(&mut zst)?;
        Ok(zst)
    }

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

        let tar = w.into_inner().await?;
        let mut zst = vec![];
        compress(tar.as_slice(), &mut zst)?;
        assert_eq!(zst[..4], [0x28, 0xb5, 0x2f, 0xfd]); // zstd's magic number
        assert_eq!(zst, compress_to_vec(tar.as_slice(), CompressionLevel::Fastest));
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

        let exported = exported().await?;
        let mut written = vec![];
        for dst in ["/a/out-3333333333333333.tar.zst", "/b/out-3333333333333333.tar.zst"] {
            spooled_at(dst, &exported).finalize("stamp = 3\n")?;
            written.push(stored(dst.into())?);
        }
        assert_eq!(written[0], written[1]);

        // As if all of it were at hand: however it's read, compression gives the same bytes
        let mut whole = exported[..exported.len() - MARKER].to_vec();
        whole.extend_from_slice(header_for(MD_TOML, "stamp = 3\n".len())?.as_bytes());
        whole.extend_from_slice(b"stamp = 3\n");
        whole.resize(whole.len().next_multiple_of(TAR_BLOCK as usize) + MARKER, 0);
        assert_eq!(written[0], compress_to_vec(whole.as_slice(), CompressionLevel::Fastest));

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

    #[test]
    fn not_being_able_to_write_a_result_is_an_error_not_a_panic() {
        let e = compress([10; 1 << 20].as_slice(), Broken).expect_err("nowhere to write");
        assert_eq!(e.kind(), io::ErrorKind::StorageFull);

        let e = compress(Broken, vec![]).expect_err("nothing to read");
        assert_eq!(e.kind(), io::ErrorKind::StorageFull);
    }

    struct Broken;

    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::StorageFull, "disk full"))
        }
    }

    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::StorageFull, "disk full"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Seek for Broken {
        fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
            Ok(0)
        }
    }

    #[tokio::test]
    async fn not_being_able_to_spool_only_loses_the_result() -> Result<()> {
        let fakefs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fakefs.clone(), ..Sys::fake() });
        let exported = exported().await?;
        let end = (exported.len() - MARKER) as u64;

        fakefs.readonly("/ro");
        let dirs = Dirs { results: "/ro".into(), buildkit: "/b".into(), var_run: "/r".into() };
        let target = Stage::new("out-3333333333333333")?;
        assert!(dirs.new_spool(&target).is_err(), "read-only");

        let spooled = Spooled {
            dst: "/results/out.tar.zst".into(),
            spool: KeepsFirstError::new(Box::new(Broken)),
        };
        let mut tee = Tee::new(exported.as_slice(), Some(spooled));
        let mut read = vec![];
        let _ = tee.read_to_end(&mut read).await?;
        assert_eq!(read, exported, "reading through is unaffected");
        let e = tee.into_result(end).err().expect("nothing got spooled").to_string();
        assert!(e.contains("Failed spooling"), "{e}");
        Ok(())
    }

    #[tokio::test]
    async fn extracting_entries_in_one_pass() -> Result<()> {
        let fakefs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fakefs.clone(), ..Sys::fake() });
        let dst: Utf8PathBuf = "/results/out-3333333333333333.tar.zst".into();

        let exported = exported().await?;
        let end = (exported.len() - MARKER) as u64;

        let dirs = Dirs { results: "/results".into(), buildkit: "/b".into(), var_run: "/r".into() };
        let target = Stage::new("out-3333333333333333")?;

        // Stops short of the end-of-archive marker
        let mut tee = Tee::new(&exported[..exported.len() - MARKER], dirs.new_spool(&target)?);
        let _ = copy(&mut tee, &mut sink()).await?;
        let e = tee.into_result(end).err().expect("no marker").to_string();
        assert!(e.contains("Truncated"), "{e}");

        let mut tee = Tee::new(exported.as_slice(), dirs.new_spool(&target)?);
        let _ = copy(&mut tee, &mut sink()).await?;
        assert_eq!(tee.count(), exported.len() as u64);
        tee.into_result(end)?.expect("not stored yet").finalize("stamp = 3")?;
        assert_eq!(fakefs.written(), std::slice::from_ref(&dst), "the spool leaves nothing behind");

        assert!(dirs.new_spool(&target)?.is_none(), "already stored");
        spooled_at(dst.as_str(), &exported).finalize("not stored")?;
        assert_eq!(fakefs.written(), std::slice::from_ref(&dst), "another build already stored it");

        let [md, missing, out] = extract(&dst, [MD_TOML, "nope", "1"]).await?;
        assert_eq!(out, b"out");
        assert_eq!(md, b"stamp = 3");
        assert!(missing.is_empty());

        // The exporter's entries come first, untouched: only its end-of-archive marker moved
        let tar = read(&dst)?;
        let mut streamed = vec![];
        let _ = stream(&dst)?.read_to_end(&mut streamed).await?;
        assert_eq!(streamed, tar);
        assert_eq!(tar[..exported.len() - MARKER], exported[..exported.len() - MARKER]);
        assert_eq!(tar[tar.len() - MARKER..], vec![0; MARKER]);
        assert!(tar.len().is_multiple_of(TAR_BLOCK as usize));
        Ok(())
    }
}
