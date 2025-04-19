use std::sync::LazyLock;

use nutype::nutype;

pub(crate) const RST: &str = "rust-base"; // Twin, for Display
pub(crate) static RUST: LazyLock<Stage> = LazyLock::new(|| Stage::try_new(RST).expect(RST));

#[test]
fn rust_stage() {
    assert_eq!(&RUST.as_ref(), "rust-base");
    assert_eq!(format!("{RST}"), "rust-base");
    assert_ne!(format!("{RUST:?}"), "rust-base");
}

// TODO newtype Image = String (better: enum)
//      for docker-image://docker.io/... (_: variant)
//          make ToString impl panic if value not fully locked

#[nutype(
    sanitize(with = oci_name),
    validate(predicate = tag_name),
    derive(Clone, Debug, Display, Deref, TryFrom, Serialize, Deserialize, Eq, PartialEq, Ord, PartialOrd))
]
pub(crate) struct Stage(String);

#[must_use]
fn oci_name(name: String) -> String {
    name.to_lowercase()
        .replace(|c: char| !is_alnum_dot_underscore(c), "-")
        .replace("---", "-")
        .to_owned()
}

#[must_use]
fn is_alnum_dot_underscore(c: char) -> bool {
    "._".contains(c) || c.is_alphanumeric()
}

#[must_use]
fn tag_name(name: &str) -> bool {
    !name.starts_with(['-', '.']) && (1..=128).contains(&name.len())
}

#[test]
fn safe_stages() {
    let mk = |x| Stage::try_new(x).unwrap().to_string();

    pretty_assertions::assert_eq!(
        mk("libgit2-sys-0.14.2+1.5.1-index.crates.io-6f17d22bba15001f"),
        "libgit2-sys-0.14.2-1.5.1-index.crates.io-6f17d22bba15001f".to_owned()
    );

    assert!(Stage::try_new("-libgit2-sys-0.14.2+1.5.1-index.crates.io-6f17d22bba15001f").is_err());
    assert!(Stage::try_new(".libgit2-sys-0.14.2+1.5.1-index.crates.io-6f17d22bba15001f").is_err());
    assert!(Stage::try_new(".libgit2-".to_owned() + &"b".repeat(128)).is_err());
}
