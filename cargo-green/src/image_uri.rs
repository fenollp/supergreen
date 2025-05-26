use std::sync::LazyLock;

use anyhow::{bail, Error, Result};
use nutype::nutype;

pub(crate) static SYNTAX: LazyLock<ImageUri> =
    LazyLock::new(|| ImageUri::try_new("docker-image://docker.io/docker/dockerfile:1.17").unwrap());

#[nutype(
    default = SYNTAX.as_str(),
    validate(error = Error, with = docker_image_uri),
    derive(Clone, Debug, Default, Display, Deref, TryFrom, Serialize, Deserialize, Eq, PartialEq, Hash))
]
pub(crate) struct ImageUri(String);

fn docker_image_uri(uri: &str) -> Result<()> {
    if uri.trim() != uri {
        bail!("Has leading or trainling whitespace: {uri:?}")
    }
    if !uri.starts_with("docker-image://") {
        bail!("Unsupported scheme: {uri:?}")
    }
    if uri.contains([' ', '\'', '"']) {
        bail!("Contains empty names, quotes or whitespace")
    }
    Ok(())
}

impl ImageUri {
    #[must_use]
    pub(crate) fn std(tagged: &str) -> Self {
        assert!(!tagged.is_empty());
        let uri = Self::try_new(format!("docker-image://docker.io/library/{tagged}")).unwrap();
        assert!(uri.tagged());
        assert!(!uri.locked());
        uri
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.as_str() == SYNTAX.as_str()
    }

    #[must_use]
    pub(crate) fn noscheme(&self) -> &str {
        self.trim_start_matches("docker-image://")
    }

    #[must_use]
    pub(crate) fn stable_syntax_frontend(&self) -> bool {
        self.starts_with(SYNTAX.as_str())
    }

    #[must_use]
    pub(crate) fn locked(&self) -> bool {
        self.contains("@sha256:")
    }

    #[must_use]
    pub(crate) fn unlocked(&self) -> Self {
        assert!(self.locked());
        self.trim_end_matches(|c| c != '@').trim_end_matches('@').try_into().unwrap()
    }

    #[must_use]
    pub(crate) fn lock(&self, sha_digest: &str) -> Self {
        assert!(!self.locked());
        assert!(sha_digest.starts_with("sha256:"));
        assert_eq!(sha_digest.len(), "sha256:".len() + 64);
        format!("{self}@{sha_digest}").try_into().unwrap()
    }

    #[must_use]
    pub(crate) fn digest(&self) -> &str {
        assert!(self.locked());
        self.trim_start_matches(|c| c != '@').trim_start_matches('@')
    }

    #[must_use]
    pub(crate) fn path_and_tag(&self) -> (&str, &str) {
        assert!(!self.locked());
        let img = self.noscheme();
        img.split_once(':').unwrap_or((img, "latest"))
    }

    #[must_use]
    pub(crate) fn tagged(&self) -> bool {
        let colons = self.noscheme().chars().filter(|c| *c == ':').count();
        colons == if self.locked() { 2 } else { 1 }
    }
}
