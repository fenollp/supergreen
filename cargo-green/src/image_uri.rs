use std::sync::LazyLock;

use anyhow::{bail, Error, Result};
use nutype::nutype;

/// Default BuildKit syntax: `docker-image://docker.io/docker/dockerfile:1`
pub(crate) static SYNTAX_IMAGE: LazyLock<ImageUri> =
    LazyLock::new(|| ImageUri::try_new("docker-image://docker.io/docker/dockerfile:1").unwrap());

/// Default BuildKit syntax, pre-locked (on 2026-04-28)
pub(crate) static SYNTAX_IMAGE_LOCKED: LazyLock<ImageUri> = LazyLock::new(|| {
    SYNTAX_IMAGE.lock("sha256:2780b5c3bab67f1f76c781860de469442999ed1a0d7992a5efdf2cffc0e3d769")
});

/// An OCI image URI of the format `docker-image://host/namespace/name:tag@sha256:digest`
///
/// * Supported scheme: `docker-image://`
/// * With or without tag.
/// * With or without digest ie. "locked" or "unlocked".
#[nutype(
    default = SYNTAX_IMAGE.as_str(),
    validate(error = Error, with = docker_image_uri),
    derive(Clone, Debug, Default, Display, Deref, TryFrom, Serialize, Deserialize, Eq, PartialEq, Ord, PartialOrd, Hash))
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
        assert!(!tagged.is_empty(), "cannot be the empty string");
        let uri = Self::try_new(format!("docker-image://docker.io/library/{tagged}")).unwrap();
        assert!(uri.tagged(), "must have a tag: {uri}");
        assert!(!uri.locked(), "must not be locked: {uri}");
        uri
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.as_str() == SYNTAX_IMAGE.as_str()
    }

    #[must_use]
    pub(crate) fn noscheme(&self) -> &str {
        self.trim_start_matches("docker-image://")
    }

    #[must_use]
    pub(crate) fn stable_syntax_frontend(&self) -> bool {
        self.starts_with(SYNTAX_IMAGE.as_str())
    }

    #[must_use]
    pub(crate) fn locked(&self) -> bool {
        self.contains("@sha256:")
    }

    #[must_use]
    pub(crate) fn unlocked(&self) -> Self {
        assert!(self.locked(), "must be locked: {self}");
        self.trim_end_matches(|c| c != '@').trim_end_matches('@').try_into().unwrap()
    }

    #[must_use]
    pub(crate) fn lock(&self, sha_digest: &str) -> Self {
        assert!(!self.locked(), "must not be locked: {self}");
        assert!(sha_digest.starts_with("sha256:"), "unknown digest algo: {sha_digest}");
        assert_eq!(sha_digest.len(), "sha256:".len() + 64, "incorrect digest length: {sha_digest}");
        format!("{self}@{sha_digest}").try_into().expect("PROOF: assembled from good parts")
    }

    #[must_use]
    pub(crate) fn digest(&self) -> &str {
        assert!(self.locked(), "must be locked: {self}");
        self.trim_start_matches(|c| c != '@').trim_start_matches('@')
    }

    #[must_use]
    pub(crate) fn path_and_tag(&self) -> (&str, &str) {
        assert!(!self.locked(), "must not be locked: {self}");
        let img = self.noscheme();
        if let Some((_, rhs)) = self.rsplit_once('/') {
            if let Some((_, tag)) = rhs.rsplit_once(':') {
                return (img.trim_end_matches(tag).trim_end_matches(':'), tag);
            }
            return (img, "latest");
        }
        (img, "latest")
    }

    #[must_use]
    pub(crate) fn tagged(&self) -> bool {
        if let Some((_, rhs)) = self.rsplit_once('/') {
            if let Some((lhs, _)) = rhs.split_once('@') {
                return lhs.contains(':');
            }
            return rhs.contains(':');
        }
        false
    }

    #[must_use]
    pub(crate) fn host(&self) -> &str {
        let img = self.noscheme();
        assert!(img.contains('/'), "must contain a path: {img}");
        let (host, _) = self.noscheme().split_once('/').expect("PROOF: just checked");
        host
    }
}

#[test]
fn imageuri_syntax() {
    assert!(!SYNTAX_IMAGE.locked());
    assert!(SYNTAX_IMAGE.tagged());
    assert!(SYNTAX_IMAGE.is_empty());
    assert!(SYNTAX_IMAGE.stable_syntax_frontend());
    assert_eq!(SYNTAX_IMAGE.host(), "docker.io");

    assert!(SYNTAX_IMAGE_LOCKED.locked());
    assert!(SYNTAX_IMAGE_LOCKED.tagged());
    assert!(!SYNTAX_IMAGE_LOCKED.is_empty());
    assert!(SYNTAX_IMAGE_LOCKED.stable_syntax_frontend());
    assert_eq!(SYNTAX_IMAGE_LOCKED.host(), "docker.io");
}

#[test]
fn imageuri_basic() {
    const DIGEST: &str = "sha256:27086352fd5e1907ea2b934eb1023f217c5ae087992eb59fde121dce9c9ff21e";

    let img = ImageUri::try_new("docker-image://registry.com/fenollp/supergreen").unwrap();
    assert!(!img.locked());
    assert!(!img.tagged());
    assert!(!img.is_empty());
    assert!(!img.stable_syntax_frontend());
    assert_eq!(img.path_and_tag(), ("registry.com/fenollp/supergreen", "latest"));
    assert_eq!(img.host(), "registry.com");
    let img = img.lock(DIGEST);
    assert!(img.locked());
    assert!(!img.tagged());
    assert!(!img.is_empty());
    assert!(!img.stable_syntax_frontend());
    assert_eq!(img.host(), "registry.com");
    assert_eq!(img.digest(), DIGEST);

    let img = ImageUri::try_new("docker-image://registry.com/fenollp/supergreen:tagged").unwrap();
    assert!(!img.locked());
    assert!(img.tagged());
    assert!(!img.is_empty());
    assert!(!img.stable_syntax_frontend());
    assert_eq!(img.path_and_tag(), ("registry.com/fenollp/supergreen", "tagged"));
    assert_eq!(img.host(), "registry.com");
    let img = img.lock(DIGEST);
    assert!(img.locked());
    assert!(img.tagged());
    assert!(!img.is_empty());
    assert!(!img.stable_syntax_frontend());
    assert_eq!(img.host(), "registry.com");
    assert_eq!(img.digest(), DIGEST);

    let img = ImageUri::try_new("docker-image://registry.com/fenollp/supergreen:tagged@sha256:27086352fd5e1907ea2b934eb1023f217c5ae087992eb59fde121dce9c9ff21e").unwrap();
    assert!(img.locked());
    assert!(img.tagged());
    assert!(!img.is_empty());
    assert!(!img.stable_syntax_frontend());
    assert_eq!(img.digest(), DIGEST);
    assert_eq!(img.host(), "registry.com");
    assert_eq!(
        img.unlocked(),
        ImageUri::try_new("docker-image://registry.com/fenollp/supergreen:tagged").unwrap()
    );

    let img = ImageUri::try_new("docker-image://registry.com/fenollp/supergreen@sha256:27086352fd5e1907ea2b934eb1023f217c5ae087992eb59fde121dce9c9ff21e").unwrap();
    assert!(img.locked());
    assert!(!img.tagged());
    assert!(!img.is_empty());
    assert!(!img.stable_syntax_frontend());
    assert_eq!(img.digest(), DIGEST);
    assert_eq!(img.host(), "registry.com");
    assert_eq!(
        img.unlocked(),
        ImageUri::try_new("docker-image://registry.com/fenollp/supergreen").unwrap()
    );
}

#[test]
fn imageuri_with_port() {
    const DIGEST: &str = "sha256:27086352fd5e1907ea2b934eb1023f217c5ae087992eb59fde121dce9c9ff21e";

    let img = ImageUri::try_new("docker-image://localhost:5000/fenollp/supergreen").unwrap();
    assert!(!img.locked());
    assert!(!img.tagged());
    assert!(!img.is_empty());
    assert!(!img.stable_syntax_frontend());
    assert_eq!(img.path_and_tag(), ("localhost:5000/fenollp/supergreen", "latest"));
    assert_eq!(img.host(), "localhost:5000");
    let img = img.lock(DIGEST);
    assert!(img.locked());
    assert!(!img.tagged());
    assert!(!img.is_empty());
    assert!(!img.stable_syntax_frontend());
    assert_eq!(img.host(), "localhost:5000");
    assert_eq!(img.digest(), DIGEST);

    let img = ImageUri::try_new("docker-image://localhost:5000/fenollp/supergreen:tagged").unwrap();
    assert!(!img.locked());
    assert!(img.tagged());
    assert!(!img.is_empty());
    assert!(!img.stable_syntax_frontend());
    assert_eq!(img.path_and_tag(), ("localhost:5000/fenollp/supergreen", "tagged"));
    assert_eq!(img.host(), "localhost:5000");
    let img = img.lock(DIGEST);
    assert!(img.locked());
    assert!(img.tagged());
    assert!(!img.is_empty());
    assert!(!img.stable_syntax_frontend());
    assert_eq!(img.host(), "localhost:5000");
    assert_eq!(img.digest(), DIGEST);

    let img = ImageUri::try_new("docker-image://localhost:5000/fenollp/supergreen:tagged@sha256:27086352fd5e1907ea2b934eb1023f217c5ae087992eb59fde121dce9c9ff21e").unwrap();
    assert!(img.locked());
    assert!(img.tagged());
    assert!(!img.is_empty());
    assert!(!img.stable_syntax_frontend());
    assert_eq!(img.digest(), DIGEST);
    assert_eq!(img.host(), "localhost:5000");
    assert_eq!(
        img.unlocked(),
        ImageUri::try_new("docker-image://localhost:5000/fenollp/supergreen:tagged").unwrap()
    );

    let img = ImageUri::try_new("docker-image://localhost:5000/fenollp/supergreen@sha256:27086352fd5e1907ea2b934eb1023f217c5ae087992eb59fde121dce9c9ff21e").unwrap();
    assert!(img.locked());
    assert!(!img.tagged());
    assert!(!img.is_empty());
    assert!(!img.stable_syntax_frontend());
    assert_eq!(img.digest(), DIGEST);
    assert_eq!(img.host(), "localhost:5000");
    assert_eq!(
        img.unlocked(),
        ImageUri::try_new("docker-image://localhost:5000/fenollp/supergreen").unwrap()
    );
}

#[test]
fn imageuri_ipv6() {
    let img = ImageUri::try_new(
        "docker-image://[2001:db8:1f70::999:de8:7648:6e8]:100/fenollp/supergreen",
    )
    .unwrap();
    assert!(!img.locked());
    assert!(!img.tagged());
    assert!(!img.is_empty());
    assert!(!img.stable_syntax_frontend());
    assert_eq!(
        img.path_and_tag(),
        ("[2001:db8:1f70::999:de8:7648:6e8]:100/fenollp/supergreen", "latest")
    );
    assert_eq!(img.host(), "[2001:db8:1f70::999:de8:7648:6e8]:100");

    let img = ImageUri::try_new(
        "docker-image://[2001:db8:1f70::999:de8:7648:6e8]:100/fenollp/supergreen:tagged",
    )
    .unwrap();
    assert!(!img.locked());
    assert!(img.tagged());
    assert!(!img.is_empty());
    assert!(!img.stable_syntax_frontend());
    assert_eq!(
        img.path_and_tag(),
        ("[2001:db8:1f70::999:de8:7648:6e8]:100/fenollp/supergreen", "tagged")
    );
    assert_eq!(img.host(), "[2001:db8:1f70::999:de8:7648:6e8]:100");

    let img = ImageUri::try_new("docker-image://[2001:db8:1f70::999:de8:7648:6e8]:100/fenollp/supergreen:tagged@sha256:27086352fd5e1907ea2b934eb1023f217c5ae087992eb59fde121dce9c9ff21e").unwrap();
    assert!(img.locked());
    assert!(img.tagged());
    assert!(!img.is_empty());
    assert!(!img.stable_syntax_frontend());
    assert_eq!(
        img.digest(),
        "sha256:27086352fd5e1907ea2b934eb1023f217c5ae087992eb59fde121dce9c9ff21e"
    );
    assert_eq!(img.host(), "[2001:db8:1f70::999:de8:7648:6e8]:100");

    let img = ImageUri::try_new("docker-image://[2001:db8:1f70::999:de8:7648:6e8]:100/fenollp/supergreen@sha256:27086352fd5e1907ea2b934eb1023f217c5ae087992eb59fde121dce9c9ff21e").unwrap();
    assert!(img.locked());
    assert!(!img.tagged());
    assert!(!img.is_empty());
    assert!(!img.stable_syntax_frontend());
    assert_eq!(
        img.digest(),
        "sha256:27086352fd5e1907ea2b934eb1023f217c5ae087992eb59fde121dce9c9ff21e"
    );
    assert_eq!(img.host(), "[2001:db8:1f70::999:de8:7648:6e8]:100");
}
