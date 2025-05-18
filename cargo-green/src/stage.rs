use std::sync::LazyLock;

use anyhow::{anyhow, bail, Error, Result};
use nutype::nutype;

pub(crate) const RST: &str = "rust-base"; // Twin, for Display
pub(crate) static RUST: LazyLock<Stage> = LazyLock::new(|| Stage::new(RST).unwrap());

#[test]
fn rust_stage() {
    assert_eq!(&RUST.as_ref(), "rust-base");
    assert_eq!(format!("{RST}"), "rust-base");
    assert_ne!(format!("{RUST:?}"), "rust-base");
}

#[nutype(
    sanitize(with = oci_name),
    validate(error = Error, with = tag_name),
    derive(Clone, Debug, Display, Deref, TryFrom, Serialize, Deserialize, Eq, PartialEq, Ord, PartialOrd))
]
pub(crate) struct Stage(String);

impl Stage {
    pub(crate) fn new(name: &str) -> Result<Self> {
        Self::try_new(name).map_err(|e| anyhow!("BUG: not stageable {name:?}: {e}"))
    }

    pub(crate) fn dep(crate_id: &str) -> Result<Self> {
        Self::new(&format!("dep-{crate_id}"))
    }

    pub(crate) fn local(crate_id: &str) -> Result<Self> {
        Self::new(&format!("pkg-{crate_id}"))
    }

    pub(crate) fn local_mount(extrafn: &str) -> Result<Self> {
        Self::new(&format!("cwd{extrafn}"))
    }

    pub(crate) fn crate_out(extrafn_nodash: &str) -> Result<Self> {
        Self::new(&format!("crate_out-{extrafn_nodash}"))
    }

    // TODO: link this to the build script it's coming from
    pub(crate) fn cratesio(name: &str, version: &str) -> Result<Self> {
        Self::new(&format!("cratesio-{name}-{version}"))
    }

    pub(crate) fn checkout(dir: &str, commit: &str) -> Result<Self> {
        Self::new(&format!("checkout-{dir}-{commit}"))
    }

    #[must_use]
    pub(crate) fn is_remote(&self) -> bool {
        self.starts_with("cratesio-") || self.starts_with("checkout-")
    }

    pub(crate) fn incremental(extrafn: &str) -> Result<Self> {
        Self::new(&format!("inc{extrafn}"))
    }

    pub(crate) fn output(extrafn_nodash: &str) -> Result<Self> {
        Self::new(&format!("out-{extrafn_nodash}"))
    }
}

fn tag_name(name: &str) -> Result<()> {
    if name.starts_with(['-', '.']) {
        bail!("Starts with dot or dash")
    }
    if !(1..=128).contains(&name.len()) {
        bail!("Is longer than 128 chars or empty")
    }
    Ok(())
}

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

#[test]
fn stages() {
    let local_mount = Stage::local_mount("-9d1546e4763fe483").unwrap();
    let crate_out = Stage::crate_out("9d1546e4763fe483").unwrap();
    let cratesio = Stage::cratesio("syn", "1.0.46").unwrap();
    let checkout =
        Stage::checkout("buildxargs-76dd4ee9dadcdcf0", "df9b810011cd416b8e3fc02911f2f496acb8475e")
            .unwrap();

    let stages = [
        (
            Stage::dep("l-buildxargs-1.4.0-b4243835fd7aaf9f").unwrap(),
            "dep-l-buildxargs-1.4.0-b4243835fd7aaf9f",
        ),
        (
            Stage::local("t-cargo-green-0.11.0-8555e1723d7ec5eb").unwrap(),
            "pkg-t-cargo-green-0.11.0-8555e1723d7ec5eb",
        ),
        (local_mount.clone(), "cwd-9d1546e4763fe483"),
        (crate_out.clone(), "crate_out-9d1546e4763fe483"),
        (cratesio.clone(), "cratesio-syn-1.0.46"),
        (
            checkout.clone(),
            "checkout-buildxargs-76dd4ee9dadcdcf0-df9b810011cd416b8e3fc02911f2f496acb8475e",
        ),
        (Stage::incremental("-9d1546e4763fe483").unwrap(), "inc-9d1546e4763fe483"),
        (Stage::output("9d1546e4763fe483").unwrap(), "out-9d1546e4763fe483"),
    ];

    for (stage, sname) in stages {
        assert_eq!(stage.to_string(), sname);
        assert_eq!(stage.is_remote(), [&cratesio, &checkout].contains(&&stage));
    }
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
