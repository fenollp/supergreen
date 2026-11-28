//! Memory-only streaming compression, hashing both sides along the way

use std::{
    io::{self, Read, Write},
    pin::Pin,
    task::{Context, Poll, ready},
};

use anyhow::{Result, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use ruzstd::encoding::{self, CompressionLevel};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, duplex},
    runtime::Handle,
    task::{JoinHandle, spawn_blocking},
};

use crate::{cache::oci::Layer, ext::hex, sys::AtomicWriter};

/// How much of what's fed is held back from compression, so it can still be cut off: where results'
/// entries end, and ours go, is only known once all of the tarball was read. Far more than the two
/// zeroed blocks the exporter ends with: more than this following the entries is a bug.
///
/// cf [`crate::cache::result::Tee::ensure_short_tail`].
pub(crate) const HELD_BACK: usize = 64 << 10;

/// Compressed bytes' writer and what it got, once all was compressed.
type Compressed = (Box<dyn AtomicWriter>, Layer);

/// How much may sit between what's fed and its compression
const PIPE_SIZE: usize = 1 << 20;

/// Bytes on their way to becoming the file at `dst`: compressed on tokio's blocking pool.
pub(crate) struct Compressing {
    dst: Utf8PathBuf,
    pipe: DuplexStream,
    task: JoinHandle<io::Result<Compressed>>,
    held: Vec<u8>,
    /// No longer held back, on its way into the pipe
    due: Vec<u8>,
    /// How much was ever no longer held back
    sent: u64,
}

impl Compressing {
    /// `zst` writes to `dst`
    ///
    /// Panics outside of a tokio runtime.
    #[must_use]
    pub(crate) fn start(dst: Utf8PathBuf, zst: Box<dyn AtomicWriter>) -> Self {
        let (pipe, compressors) = duplex(PIPE_SIZE);
        let from = Piped { pipe: compressors, runtime: Handle::current() };
        let task = spawn_blocking(move || {
            let mut raw = Hashed::new(from);
            let mut hashed = Hashed::new(zst);
            compress(&mut raw, &mut hashed)?;
            let Hashed { inner: zst, hasher, count: size } = hashed;
            Ok((zst, Layer { digest: hex(hasher), size, diff_id: hex(raw.hasher) }))
        });
        Self { dst, pipe, task, held: vec![], due: vec![], sent: 0 }
    }

    #[must_use]
    pub(crate) fn dst(&self) -> &Utf8Path {
        &self.dst
    }

    /// Moves what's due into the pipe, as it makes room: a slow compression slows reading down,
    /// instead of filling memory.
    ///
    /// Fails only if compression is gone: [`Self::finish`] tells.
    pub(crate) fn poll_send(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        while !self.due.is_empty() {
            match ready!(Pin::new(&mut self.pipe).poll_write(cx, &self.due)) {
                Ok(n) => drop(self.due.drain(..n)),
                Err(_) => self.due.clear(),
            }
        }
        Poll::Ready(())
    }

    /// [`Self::poll_send`] first, for what's due not to pile up.
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        self.held.extend_from_slice(bytes);
        if self.held.len() >= 2 * HELD_BACK {
            let n = self.held.len() - HELD_BACK;
            self.due.extend(self.held.drain(..n));
            self.sent += n as u64;
        }
    }

    /// Forgets what was pushed past the first `len` bytes. `false` if too late for that: some of it
    /// is already being compressed, or `len` doesn't fit what's held.
    #[must_use]
    pub(crate) fn cut_at(&mut self, len: u64) -> bool {
        let kept = len.checked_sub(self.sent);
        let Some(kept) = kept.and_then(|n| usize::try_from(n).ok()) else { return false };
        self.held.truncate(kept);
        true
    }

    /// Compresses what's held back, then `tail`. Committing the writer is left to the caller.
    pub(crate) async fn finish(self, tail: Vec<u8>) -> Result<Compressed> {
        let Self { dst, mut pipe, task, held, due, .. } = self;
        for chunk in [due, held, tail] {
            // Fails only if compression is gone: the task tells
            let _ = pipe.write_all(&chunk).await;
        }
        drop(pipe); // The end
        task.await
            .map_err(|e| anyhow!("BUG: compression of {dst} did not finish: {e}"))?
            .map_err(|e| anyhow!("Failed compressing {dst}: {e}"))
    }
}

/// A pipe's reading end, for [`spawn_blocking`] tasks.
struct Piped {
    pipe: DuplexStream,
    runtime: Handle,
}

impl Read for Piped {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.runtime.block_on(self.pipe.read(buf))
    }
}

/// Hashes and counts what goes through.
struct Hashed<T> {
    inner: T,
    hasher: Sha256,
    count: u64,
}

impl<T> Hashed<T> {
    fn new(inner: T) -> Self {
        Self { inner, hasher: Sha256::new(), count: 0 }
    }

    fn update(&mut self, bytes: &[u8]) {
        self.hasher.update(bytes);
        self.count += bytes.len() as u64;
    }
}

impl<R: Read> Read for Hashed<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.update(&buf[..n]);
        Ok(n)
    }
}

impl<W: Write> Write for Hashed<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

// TODO: checkup on (Implement the Default and Better compression levels)[https://github.com/KillingSpark/zstd-rs/pull/135]
pub(crate) fn compress(from: impl Read, into: impl Write) -> io::Result<()> {
    let (mut from, mut into) = (KeepsFirstError::new(from), KeepsFirstError::new(into));
    // The only level implemented besides `Uncompressed`: on results, ~3x faster than gzip's
    // default level, though ~50% bigger. No checksum (ruzstd's `hash` feature): less `unsafe`.
    encoding::compress(&mut from, &mut into, CompressionLevel::Fastest);
    from.error.or(into.error).map_or(Ok(()), Err)
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

#[cfg(test)]
pub(crate) mod tests {
    use std::io::{self, Read, Write};

    use super::compress;
    use crate::sys::AtomicWriter;

    #[test]
    fn not_being_able_to_write_is_an_error_not_a_panic() {
        let e = compress([10; 1 << 20].as_slice(), Broken).expect_err("nowhere to write");
        assert_eq!(e.kind(), io::ErrorKind::StorageFull);

        let e = compress(Broken, vec![]).expect_err("nothing to read");
        assert_eq!(e.kind(), io::ErrorKind::StorageFull);
    }

    pub(crate) struct Broken;

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

    impl AtomicWriter for Broken {
        fn commit(self: Box<Self>) -> io::Result<()> {
            unreachable!("nothing to commit")
        }
    }
}
