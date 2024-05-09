use nutype::nutype;

#[nutype(sanitize(with=oci_name), validate(not_empty), derive(Debug, Display, PartialEq))]
pub(crate) struct Stage(String);

#[inline]
#[must_use]
fn oci_name(name: String) -> String {
    name.to_lowercase()
        .replace(|c: char| c != '.' && !c.is_alphanumeric(), "-")
        .replace("___", "_")
        .to_owned()
}

#[test]
fn safe_stages() {
    pretty_assertions::assert_eq!(
        Stage::new("libgit2-sys-0.14.2+1.5.1-index.crates.io-6f17d22bba15001f".to_owned())
            .unwrap()
            .to_string(),
        "libgit2-sys-0.14.2-1.5.1-index.crates.io-6f17d22bba15001f".to_owned()
    );
}
