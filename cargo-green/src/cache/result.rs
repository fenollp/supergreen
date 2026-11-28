//! Build results are the artifacts of the runner's `rustc` (and build scripts) invocations:
//! the tarball BuildKit's tar exporter produced, with our `md.toml` appended to it.

use std::{
    io::{self, Read, Write},
    pin::Pin,
    task::{Context, Poll, ready},
};

use anyhow::{Result, anyhow, bail};
use async_tar::{Archive as TarArchive, Entry as TarEntry, EntryType, Header};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info};
use ruzstd::decoding::StreamingDecoder;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::{
    build::SOURCE_DATE_EPOCH,
    cache::compressing::Compressing,
    dirs::Dirs,
    stage::Stage,
    sys::{TAR_BLOCK, TAR_MODE, fs},
};

/// Results' own entry, appended last.
///
/// Can't clash with what the tar exporter produces: the stdio files and the out dir.
pub(crate) const MD_TOML: &str = "md.toml";

impl Dirs {
    pub(crate) fn result_from_stage(&self, target: &Stage) -> Utf8PathBuf {
        self.results.join(format!("{}.tar.zst", target.bucketed()))
    }

    /// What building this stage's result cost: kept out of it for its bytes not to vary.
    pub(crate) fn stats_from_stage(&self, target: &Stage) -> Utf8PathBuf {
        self.stats.join(format!("{}.toml", target.bucketed()))
    }

    /// Where to pipe the tarball to, unless the result is stored already
    pub(crate) fn new_result(&self, target: &Stage) -> Result<Option<Compressing>> {
        let dst = self.result_from_stage(target);
        if fs().exists(&dst) {
            return Ok(None);
        }
        // NOTE: TOCTOU on dst is okay as long as writing it is atomic
        if let Some(bucket) = dst.parent() {
            fs().create_dir_all(bucket)
                .map_err(|e| anyhow!("Failed to `mkdir -p {bucket}`: {e}"))?;
        }
        let zst = fs()
            .atomic_writer(&dst, TAR_MODE)
            .map_err(|e| anyhow!("Failed opening (Watomic) result {dst}: {e}"))?;
        Ok(Some(Compressing::start(dst, zst)))
    }
}

/// Compresses what's read through it, if it can: not being able to store a result doesn't fail a
/// build.
pub(crate) struct Tee<R> {
    inner: R,
    compressing: Option<Compressing>,
    count: u64,
}

impl<R> Tee<R> {
    #[must_use]
    pub(crate) fn new(inner: R, compressing: Option<Compressing>) -> Self {
        Self { inner, compressing, count: 0 }
    }

    /// How many bytes went through
    #[must_use]
    pub(crate) fn count(&self) -> u64 {
        self.count
    }

    /// Given where entries end: where the end-of-archive marker starts, and where ours goes.
    pub(crate) fn into_result(self, entries_end: u64) -> Result<Option<ResultWriter>> {
        let Some(mut compressing) = self.compressing else { return Ok(None) };
        if entries_end == 0 || self.count < entries_end + 2 * TAR_BLOCK {
            bail!("Truncated tarball: {}B, entries end at {entries_end}B", self.count)
        }
        if !compressing.cut_at(entries_end) {
            bail!("Unexpected tarball: {}B after its entries", self.count - entries_end)
        }
        Ok(Some(ResultWriter { compressing }))
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for Tee<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        // Room first: what gets read can't be given back
        if let Some(compressing) = &mut me.compressing {
            ready!(compressing.poll_send(cx));
        }
        let before = buf.filled().len();
        let polled = Pin::new(&mut me.inner).poll_read(cx, buf);
        let read = &buf.filled()[before..];
        me.count += read.len() as u64;
        if let Some(compressing) = &mut me.compressing {
            compressing.push(read);
        }
        polled
    }
}

/// The most a tar header's own (11 octal digits) size field holds: 8GiB - 1.
/// Anything bigger comes with a PAX `size` record, or in GNU's base-256.
const MAX_HEADER_SIZE: u64 = 0o77_777_777_777;

/// Where this entry ends: where the next one, or the end-of-archive marker, starts.
///
/// Fails on entries of 8GiB and more: the exporter has no reason to write any, and these have
/// their size in PAX records, which these positions don't account for.
/// Better to stop (and be told about it) than to produce a corrupted result.
pub(crate) async fn entry_end<R: AsyncRead + Unpin>(
    f: &mut TarEntry<TarArchive<R>>,
) -> Result<u64> {
    let name = f.path().map_err(|e| anyhow!("Bad name in tarball: {e}"))?;
    let name = name.to_string_lossy().to_string();

    let records = f.pax_extensions().await;
    let records = records.map_err(|e| anyhow!("Failed reading PAX records of {name}: {e}"))?;
    if let Some(records) = records
        && records
            .into_iter()
            .filter_map(std::result::Result::ok)
            .find(|r| r.key_bytes() == b"size")
            .is_some()
    {
        bail!(
            "BUG: entry {name} overrides its size through an unsupported PAX record: please report this"
        )
    }

    let size = f.header().entry_size();
    let size = size.map_err(|e| anyhow!("Failed reading the size of {name}: {e}"))?;
    if size >= MAX_HEADER_SIZE {
        bail!("BUG: entry {name} is of >=8GiB ({size}B): please report this")
    }

    // NOTE: an entry's PAX or GNU long name headers come before its own: they're counted in
    Ok(f.raw_file_position() + size.next_multiple_of(TAR_BLOCK))
}

/// The tar exporter's tarball, compressed straight to disk up to its end-of-archive marker, then
/// completed with our entry.
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
    compressing: Compressing,
}

impl ResultWriter {
    /// No-op when another build already stored the result.
    ///
    /// Returns `true` once tarball wrote successfully.
    pub(crate) async fn finalize(self, md_ser: &str) -> Result<bool> {
        let Self { compressing } = self;
        let dst = compressing.dst().to_owned();

        macro_rules! check {
            () => {{
                if fs().exists(&dst) {
                    debug!("{dst} already exists, dropping work");
                    return Ok(false);
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

        info!("compressing the rest of result");
        let zst = compressing.finish(ours).await?;

        check!();
        info!("writing result to {dst}");
        if let Err(e) = zst.commit() {
            bail!("Failed writing result {dst}: {e}")
        }
        Ok(true)
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
    match u64::try_from(len) {
        Ok(n) if n < MAX_HEADER_SIZE => header.set_size(n),
        _ => bail!("BUG: tarball too big: {len}B (>={MAX_HEADER_SIZE})"),
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
    use std::io::Read;

    use anyhow::{Result, anyhow};
    use async_tar::{Archive as TarArchive, Builder as TarBuilder, EntryType, Header};
    use camino::{Utf8Path, Utf8PathBuf};
    use ruzstd::{
        decoding::StreamingDecoder,
        encoding::{CompressionLevel, compress_to_vec},
    };
    use tokio::io::{AsyncReadExt, copy, sink};
    use tokio_stream::StreamExt;

    use super::{
        MAX_HEADER_SIZE, MD_TOML, ResultWriter, Tee, assert_tarball_header, entry_end, header_for,
        stream,
    };
    use crate::{
        cache::compressing::{Compressing, HELD_BACK, compress, tests::Broken},
        dirs::Dirs,
        md::MdId,
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

    async fn compressed_at(dst: &str, exported: &[u8]) -> Result<ResultWriter> {
        let zst = fs().atomic_writer(dst.into(), 0o644)?;
        let compressing = Compressing::start(dst.into(), zst);
        let mut tee = Tee::new(exported, Some(compressing));
        let _ = copy(&mut tee, &mut sink()).await?;
        Ok(tee.into_result((exported.len() - MARKER) as u64)?.expect("compressing"))
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
        let ar = TarArchive::new(tar.as_slice());
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

    async fn end_of_first_entry(tar: Vec<u8>) -> Result<u64> {
        let ar = TarArchive::new(tar.as_slice());
        let mut entries = ar.entries()?;
        let mut f = entries.next().await.expect("an entry")?;
        entry_end(&mut f).await
    }

    #[tokio::test]
    async fn entries_end_after_their_padding() -> Result<()> {
        let mut w = TarBuilder::new(vec![]);
        w.append(&header_for("a", 3)?, &[1u8, 2, 3][..]).await?;
        assert_eq!(end_of_first_entry(w.into_inner().await?).await?, 2 * TAR_BLOCK);
        Ok(())
    }

    #[tokio::test]
    async fn entries_of_8gib_and_more_are_refused_with_a_pax_size_record() -> Result<()> {
        let records = b"20 size=99999999999\n";
        let mut pax = Header::new_gnu();
        pax.set_path("pax")?;
        pax.set_entry_type(EntryType::XHeader);
        pax.set_size(records.len() as u64);
        pax.set_cksum();

        let mut w = TarBuilder::new(vec![]);
        w.append(&pax, &records[..]).await?;
        w.append(&header_for("big", 3)?, &[1u8, 2, 3][..]).await?;
        let err = end_of_first_entry(w.into_inner().await?).await.unwrap_err();
        assert!(err.to_string().starts_with("BUG: "), "{err}");
        Ok(())
    }

    #[tokio::test]
    async fn entries_of_8gib_and_more_are_refused_with_a_big_header_size() -> Result<()> {
        let mut header = header_for("big", 3)?;
        header.set_size(MAX_HEADER_SIZE);
        header.set_cksum();
        let mut tar = header.as_bytes().to_vec();
        tar.extend([0; 512]);
        let err = end_of_first_entry(tar).await.unwrap_err();
        assert!(err.to_string().starts_with("BUG: "), "{err}");
        Ok(())
    }

    #[test]
    fn we_never_write_entries_of_8gib_and_more() {
        let err = header_for("big", usize::try_from(MAX_HEADER_SIZE).unwrap()).unwrap_err();
        assert!(err.to_string().starts_with("BUG: "), "{err}");
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
        let r = TarArchive::new(buf.as_slice());

        let mut entries = r.entries()?;
        let entry = entries.next().await.expect("we wrote 1 entry");
        let mut entry = entry?;
        let header_bis = entry.header();
        assert_tarball_header(header_bis);

        assert_eq!(header.as_bytes(), header_bis.as_bytes());
        assert_eq!(header.path().ok(), header_bis.path().ok());
        assert_eq!(header.link_name().ok(), header_bis.link_name().ok());
        assert_eq!(header.entry_size().ok(), header_bis.entry_size().ok());
        assert_eq!(header.size().ok(), header_bis.size().ok());
        assert_eq!(entry.header().entry_size().unwrap(), header.entry_size().unwrap());

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
            let written_by = compressed_at(dst, &exported).await?;
            written_by.finalize("stamp = 3\n").await?;
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
        let ar = TarArchive::new(tar.as_slice());
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
    async fn not_being_able_to_write_only_loses_the_result() -> Result<()> {
        let fakefs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fakefs.clone(), ..Sys::fake() });
        let exported = exported().await?;
        let end = (exported.len() - MARKER) as u64;

        fakefs.readonly("/ro");
        let dirs = Dirs { results: "/ro".into(), ..Dirs::faked() };
        let target = Stage::new("out-3333333333333333")?;
        assert!(dirs.new_result(&target).is_err(), "read-only");

        let compressing = Compressing::start("/results/out.tar.zst".into(), Box::new(Broken));
        let mut tee = Tee::new(exported.as_slice(), Some(compressing));
        let mut read = vec![];
        let _ = tee.read_to_end(&mut read).await?;
        assert_eq!(read, exported, "reading through is unaffected");
        let result = tee.into_result(end)?.expect("compressing");
        let e = result.finalize("stamp = 3").await.unwrap_err().to_string();
        assert!(e.contains("Failed compressing"), "{e}");
        Ok(())
    }

    #[tokio::test]
    async fn tarballs_bigger_than_what_is_held_back() -> Result<()> {
        let fakefs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fakefs.clone(), ..Sys::fake() });

        let data: Vec<u8> = (0..20 * HELD_BACK + 7).map(|i| (i * 31 % 251) as u8).collect();
        let mut tar = TarBuilder::new(vec![]);
        tar.append(&header_for("big", data.len())?, data.as_slice()).await?;
        tar.finish().await?;
        let exported = tar.into_inner().await?;

        let dst = "/results/out.tar.zst";
        let _ = compressed_at(dst, &exported).await?.finalize("stamp = 3").await?;
        let mut whole = exported[..exported.len() - MARKER].to_vec();
        whole.extend_from_slice(header_for(MD_TOML, "stamp = 3".len())?.as_bytes());
        whole.extend_from_slice(b"stamp = 3");
        whole.resize(whole.len().next_multiple_of(TAR_BLOCK as usize) + MARKER, 0);
        assert_eq!(read(dst.into())?, whole);
        assert_eq!(
            stored(dst.into())?,
            compress_to_vec(whole.as_slice(), CompressionLevel::Fastest)
        );

        // Entries ending before what's already compressed can't be completed anymore
        let mut padded = exported.clone();
        padded.resize(padded.len() + 3 * HELD_BACK, 0);
        let zst = fs().atomic_writer("/results/padded.tar.zst".into(), 0o644)?;
        let compressing = Compressing::start("/results/padded.tar.zst".into(), zst);
        let mut tee = Tee::new(padded.as_slice(), Some(compressing));
        let _ = copy(&mut tee, &mut sink()).await?;
        let end = (exported.len() - MARKER) as u64;
        let e = tee.into_result(end).err().expect("too far").to_string();
        assert!(e.contains("Unexpected tarball"), "{e}");
        assert!(!fs().exists("/results/padded.tar.zst".into()));
        Ok(())
    }

    #[tokio::test]
    async fn extracting_entries_in_one_pass() -> Result<()> {
        let fakefs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fakefs.clone(), ..Sys::fake() });
        let dst: Utf8PathBuf = "/results/33/out-3333333333333333.tar.zst".into();

        let exported = exported().await?;
        let end = (exported.len() - MARKER) as u64;

        let dirs = Dirs::faked();
        let target = Stage::new("out-3333333333333333")?;

        // Stops short of the end-of-archive marker
        let mut tee = Tee::new(&exported[..exported.len() - MARKER], dirs.new_result(&target)?);
        let _ = copy(&mut tee, &mut sink()).await?;
        let e = tee.into_result(end).err().expect("no marker").to_string();
        assert!(e.contains("Truncated"), "{e}");

        let mut tee = Tee::new(exported.as_slice(), dirs.new_result(&target)?);
        let _ = copy(&mut tee, &mut sink()).await?;
        assert_eq!(tee.count(), exported.len() as u64);
        tee.into_result(end)?.expect("not stored yet").finalize("stamp = 3").await?;
        assert_eq!(fakefs.written(), std::slice::from_ref(&dst), "nothing else gets left behind");

        assert!(dirs.new_result(&target)?.is_none(), "already stored");
        compressed_at(dst.as_str(), &exported).await?.finalize("not stored").await?;
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

    #[test]
    fn entries_are_bucketed_by_hash() {
        let _guard = Sys::install(Sys { fs: FakeFs::new(), ..Sys::fake() });
        let dirs = Dirs::faked();
        let id = MdId::from("eb09e02c579e5fe1");

        let out = Stage::output(id);
        assert_eq!(dirs.result_from_stage(&out), "/results/eb/out-eb09e02c579e5fe1.tar.zst");
        assert_eq!(dirs.stats_from_stage(&out), "/stats/eb/out-eb09e02c579e5fe1.toml");

        let cache = dirs.new_runner_cache(&Stage::incremental(id)).unwrap();
        assert_eq!(cache.unwrap(), "/buildkit/eb/inc-eb09e02c579e5fe1");
    }

    #[test]
    #[should_panic(expected = "BUG")]
    fn cant_bucket_nonhashed() {
        let _ = Stage::new("short").unwrap().bucketed();
    }
}
