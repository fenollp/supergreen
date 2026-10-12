use std::{iter, sync::LazyLock};

use anyhow::{Error, Result, anyhow, bail};
use camino::Utf8PathBuf;
use nutype::nutype;
use serde::{Deserialize, Serialize};

use crate::{checkouts, cratesio, md::MdId, relative};

pub(crate) const RST: &str = "rust-base"; // Twin, for Display
pub(crate) static RUST: LazyLock<Stage> = LazyLock::new(|| Stage::new(RST).unwrap());

#[test]
fn rust_stage() {
    assert_eq!(RUST.as_str(), "rust-base");
    assert_eq!(RUST.as_str(), RST);
    assert_ne!(format!("{RUST:?}"), "rust-base");
    assert_ne!(format!("{RUST:#?}"), "rust-base");
}

#[nutype(
    sanitize(with = oci_name),
    validate(error = Error, with = tag_name),
    derive(Clone, Debug, Display, Deref, TryFrom, Serialize, Deserialize, Eq, PartialEq, Ord, PartialOrd, Hash))
]
pub(crate) struct Stage(String);

impl Stage {
    pub(crate) fn new(name: &str) -> Result<Self> {
        Self::try_new(name).map_err(|e| anyhow!("BUG: not stageable {name:?}: {e}"))
    }

    pub(crate) fn dep(crate_id: &str) -> Result<Self> {
        Self::new(&format!("dep-{crate_id}"))
    }

    pub(crate) fn local(metadata: MdId) -> Self {
        Self::new(&format!("cwd-{metadata}")).expect("PROOF: hex is valid")
    }

    #[must_use]
    pub(crate) fn is_local(&self) -> bool {
        self.starts_with("cwd-")
    }

    #[must_use]
    pub(crate) fn is_remote(&self) -> bool {
        self.starts_with("cratesio-") || self.starts_with("checkout-")
    }

    pub(crate) fn cratesio(name_dash_version: &str) -> Result<Self> {
        Self::new(&format!("cratesio-{name_dash_version}"))
    }

    pub(crate) fn checkout(dir: &str, commit: &str) -> Result<Self> {
        Self::new(&format!("checkout-{dir}-{commit}"))
    }

    pub(crate) fn incremental(metadata: MdId) -> Self {
        Self::new(&format!("inc-{metadata}")).expect("PROOF: hex is valid")
    }

    pub(crate) fn output(metadata: MdId) -> Self {
        Self::new(&format!("out-{metadata}")).expect("PROOF: hex is valid")
    }
}

pub(crate) trait AsStage: AsBlock {
    fn name(&self) -> &Stage;
    /// RUN mounts of a stage: (source within stage, destination, swappity)
    fn mounts(&self) -> impl IntoIterator<Item = (Option<Utf8PathBuf>, Utf8PathBuf, bool)> {
        iter::empty()
    }
    fn context(&mut self) -> Option<(Stage, Utf8PathBuf)> {
        None
    }
}

pub(crate) trait AsBlock {
    fn as_block(&self) -> Option<String> {
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub(crate) struct Script {
    pub(crate) stage: Stage,
    pub(crate) script: String,
}

impl AsBlock for Script {
    fn as_block(&self) -> Option<String> {
        Some(self.script.clone())
    }
}

impl AsStage for Script {
    fn name(&self) -> &Stage {
        &self.stage
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub(crate) enum NamedStage {
    Script(Script),
    Cratesio(cratesio::Cratesio),
    Checkouts(checkouts::Checkouts),
    Relative(relative::Relative),
}

impl NamedStage {
    pub(crate) fn is_rust(&self) -> bool {
        *self.name() == *RUST
    }

    /// Md (TOML) + as_block + RUN mounts
    #[cfg(test)]
    pub(crate) fn describe(&self) -> String {
        let mut txt = toml::to_string_pretty(self).unwrap();
        txt.push_str("\n# as_block\n");
        txt.push_str(self.as_block().as_deref().map_or("(none)", str::trim));
        txt.push_str("\n\n# mounts\n");
        for (src, dst, _) in self.mounts() {
            txt.push_str(&format!("{} -> {dst}\n", src.as_deref().map_or("(all)", |s| s.as_str())));
        }
        txt
    }
}

impl AsBlock for NamedStage {
    fn as_block(&self) -> Option<String> {
        match self {
            Self::Script(stage) => stage.as_block(),
            Self::Cratesio(stage) => stage.as_block(),
            Self::Checkouts(stage) => stage.as_block(),
            Self::Relative(stage) => stage.as_block(),
        }
    }
}

impl AsStage for NamedStage {
    fn name(&self) -> &Stage {
        match self {
            Self::Script(stage) => stage.name(),
            Self::Cratesio(stage) => stage.name(),
            Self::Checkouts(stage) => stage.name(),
            Self::Relative(stage) => stage.name(),
        }
    }

    fn mounts(&self) -> impl IntoIterator<Item = (Option<Utf8PathBuf>, Utf8PathBuf, bool)> {
        // Each variant's mounts() is its own opaque type: chain all four (only one being Some)
        let (mut script, mut cratesio, mut checkouts, mut relative) = (None, None, None, None);
        match self {
            Self::Script(stage) => script = Some(stage.mounts()),
            Self::Cratesio(stage) => cratesio = Some(stage.mounts()),
            Self::Checkouts(stage) => checkouts = Some(stage.mounts()),
            Self::Relative(stage) => relative = Some(stage.mounts()),
        }
        (script.into_iter().flatten())
            .chain(cratesio.into_iter().flatten())
            .chain(checkouts.into_iter().flatten())
            .chain(relative.into_iter().flatten())
    }

    fn context(&mut self) -> Option<(Stage, Utf8PathBuf)> {
        match self {
            Self::Script(stage) => stage.context(),
            Self::Cratesio(stage) => stage.context(),
            Self::Checkouts(stage) => stage.context(),
            Self::Relative(stage) => stage.context(),
        }
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
    let mut name = name.to_lowercase().replace(|c: char| !is_alnum_dot_underscore(c), "-");
    loop {
        let len = name.len();
        name = name.replace("---", "-");
        if name.len() == len {
            return shortened(name);
        }
    }
}

/// BuildKit forbids names longer than 128 chars: keep start + end with hash of whole
#[must_use]
fn shortened(name: String) -> String {
    const MAX: usize = 128;
    if name.len() <= MAX {
        return name;
    }
    let hash = crate::dirs::hash(&name);
    let budget = MAX - "-".len() - hash.len();
    let cut = name.floor_char_boundary(budget);
    format!("{}-{hash}", name[..cut].trim_end_matches('-'))
}

#[must_use]
fn is_alnum_dot_underscore(c: char) -> bool {
    "._".contains(c) || c.is_alphanumeric()
}

#[test]
fn stages() {
    let local = Stage::local(MdId::new("-9d1546e4763fe483"));
    let cratesio = Stage::cratesio("syn-1.0.46").unwrap();
    let checkout =
        Stage::checkout("buildxargs-76dd4ee9dadcdcf0", "df9b810011cd416b8e3fc02911f2f496acb8475e")
            .unwrap();

    let pairs = [
        (
            Stage::dep("l-buildxargs-1.4.0-b4243835fd7aaf9f").unwrap(),
            "dep-l-buildxargs-1.4.0-b4243835fd7aaf9f",
        ),
        (local.clone(), "cwd-9d1546e4763fe483"),
        (cratesio.clone(), "cratesio-syn-1.0.46"),
        (
            checkout.clone(),
            "checkout-buildxargs-76dd4ee9dadcdcf0-df9b810011cd416b8e3fc02911f2f496acb8475e",
        ),
        (Stage::incremental(MdId::new("-9d1546e4763fe483")), "inc-9d1546e4763fe483"),
        (Stage::output(MdId::new("-9d1546e4763fe483")), "out-9d1546e4763fe483"),
    ];

    for (stage, sname) in pairs {
        assert_eq!(stage.to_string(), sname);

        assert_eq!(stage.is_remote(), [&cratesio, &checkout].contains(&&stage));
        assert_eq!(!stage.is_remote(), ![&cratesio, &checkout].contains(&&stage));

        assert_eq!(stage.is_local(), [&local].contains(&&stage));
        assert_eq!(!stage.is_local(), ![&local].contains(&&stage));
    }
}

#[test]
fn safe_stages() {
    let mk = |x| Stage::try_new(x).unwrap().to_string();

    pretty_assertions::assert_eq!(
        mk("libgit2-sys-0.14.2+1.5.1-index.crates.io-6f17d22bba15001f"),
        "libgit2-sys-0.14.2-1.5.1-index.crates.io-6f17d22bba15001f".to_owned()
    );

    pretty_assertions::assert_eq!(&mk("cargo-c-0.10.18+-+x"), "cargo-c-0.10.18-x");
    pretty_assertions::assert_eq!(&mk("a-----a-0.0.0-0-+---a"), "a-a-0.0.0-0-a");
    pretty_assertions::assert_eq!(mk(&mk("a-----a")), mk("a-----a"));

    assert!(Stage::try_new("-libgit2-sys-0.14.2+1.5.1-index.crates.io-6f17d22bba15001f").is_err());
    assert!(Stage::try_new(".libgit2-sys-0.14.2+1.5.1-index.crates.io-6f17d22bba15001f").is_err());
    assert!(Stage::try_new(".libgit2-".to_owned() + &"b".repeat(128)).is_err());
}

#[test]
fn stage_names_get_shortened() {
    let name = "a".repeat(64); // As long as crates.io allows
    let version = "1.0.0-alpha.20250101.0123456789abcdef01234"; // 42
    let crate_id = format!("n-{name}-{version}-0123456789abcdef");
    let stage = Stage::dep(&crate_id).unwrap();
    assert_eq!(stage.len(), 128);
    assert!(stage.starts_with(&format!("dep-n-{name}-1.0.0-alpha.")), "{stage}");
    assert_eq!(Stage::new(&stage).unwrap(), stage);
    let other = Stage::dep(&crate_id.replace("0123456789abcdef", "0123456789abcdee")).unwrap();
    assert_ne!(stage, other);
    // Not cut in the middle of a multi-byte char, nor ending with dashes
    let stage = Stage::new(&format!("{}-{}", "é".repeat(60), "-".repeat(20))).unwrap();
    assert!(stage.len() <= 128, "{stage}");
}

#[cfg(test)]
mod props {
    use proptest::prelude::*;

    use super::Stage;
    use crate::md::MdId;

    /// As BuildKit's Dockerfile frontend wants them.
    fn valid(name: &str) -> bool {
        name.starts_with(|c: char| c.is_ascii_lowercase())
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_.".contains(c))
    }

    proptest! {
        /// Any crate crates.io accepts: a name of at most 64 chars, any semver version. Names it
        /// makes are valid, and stay as they are.
        #[test]
        fn stages_are_valid_names(
            kind in "[nxz]",
            name in "[A-Za-z][A-Za-z0-9_-]{0,63}",
            version in "[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}(-[0-9A-Za-z.-]{1,20})?(\\+[0-9A-Za-z.-]{1,20})?",
            n in any::<u64>(),
            commit in "[0-9a-f]{40}",
            dir in "[a-z0-9_.-]{1,40}-[0-9a-f]{16}",
        ) {
            // `dep-x-`, `-`, `-` and the MdId take 24 of the 128
            prop_assume!(name.len() + version.len() <= 104); // See stage_names_can_be_too_long
            let mdid = MdId::from(n);
            let name_dash_version = format!("{name}-{version}");
            for stage in [
                Stage::dep(&format!("{kind}-{name_dash_version}-{mdid}")),
                Stage::cratesio(&name_dash_version),
                Stage::checkout(&dir, &commit),
                Ok(Stage::local(mdid)),
                Ok(Stage::output(mdid)),
            ] {
                let stage = stage.unwrap();
                prop_assert!(valid(&stage), "{stage}");
                prop_assert_eq!(Stage::new(&stage).unwrap(), stage.clone(), "a stage name is its own name");
            }
        }
    }
}
