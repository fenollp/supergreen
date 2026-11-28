use sha2::{Digest, Sha256};

/// Lowercase hex of the SHA-256 of `data`
#[must_use]
pub(crate) fn sha256_hex(data: impl AsRef<[u8]>) -> String {
    hex(Sha256::new_with_prefix(data))
}

/// Lowercase hex of what was hashed
#[must_use]
pub(crate) fn hex(hasher: Sha256) -> String {
    // Byte by byte: not all versions of sha2 can format what they return
    hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) trait Popped: Clone {
    #[must_use]
    fn pop(&mut self) -> bool;

    #[must_use]
    fn popped(&mut self, times: usize) -> Self
    where
        Self: Sized,
    {
        for _ in 0..times {
            assert!(self.pop());
        }
        self.to_owned()
    }
}

impl Popped for camino::Utf8PathBuf {
    fn pop(&mut self) -> bool {
        self.pop()
    }
}

impl Popped for std::path::PathBuf {
    fn pop(&mut self) -> bool {
        self.pop()
    }
}

#[test]
fn sha256_in_lowercase_hex() {
    assert_eq!(
        sha256_hex("abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
