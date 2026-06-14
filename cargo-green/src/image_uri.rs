use std::{
    error::Error as StdError,
    sync::{LazyLock, Once},
    time::Duration,
};

use anyhow::{anyhow, bail, Error, Result};
use camino::Utf8Path;
use log::{info, warn};
use nutype::nutype;
use reqwest::{Client as ReqwestClient, Request};
use serde::Deserialize;
use tokio::time::sleep;

use crate::{
    du::lock_from_builder_cache,
    ext::CommandExt,
    green::Green,
    runner::{Runner, DOCKER_HOST},
};

pub(crate) const BAD_CHARS: &[char] = &[' ', '\'', '"', ';', '\\', ','];

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
    if uri.contains(BAD_CHARS) {
        bail!("Contains empty names, whitespace, quotes or bad characters")
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

    /// Returns true if image is default (unlocked syntax image)
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
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

impl Green {
    /// Read digest from builder cache, then maybe from default cache.
    ///
    /// No-op for an already locked image URI.
    ///
    /// Goal is to have a completely offline mode by default, after a `cargo green fetch`.
    pub(crate) async fn maybe_lock_image(&self, img: &ImageUri) -> Result<ImageUri> {
        if img.locked() {
            return Ok(img.to_owned());
        }
        let errer = |e| anyhow!("Failed locking {img}: {e}");
        if let Some(locked) = self.maybe_lock_from_builder_cache(img).await.map_err(errer)? {
            return Ok(locked);
        }
        if let Some(locked) = self.maybe_lock_from_image_cache(img).await.map_err(errer)? {
            return Ok(locked);
        }
        Ok(img.to_owned())
    }

    /// Reads from builder build cache if any, and falls back to image cache.
    ///
    /// <https://docs.docker.com/reference/cli/docker/buildx/imagetools/inspect/>
    ///
    /// ```text
    /// docker buildx imagetools inspect --format='{{json .Manifest.Digest}}' img.noscheme()
    /// # Only fetches remote though, and takes ages compared to fetch_digest!
    /// ```
    /// See [Getting an image's digest fast, within a docker-container builder](https://github.com/docker/buildx/discussions/3363)
    async fn maybe_lock_from_builder_cache(&self, img: &ImageUri) -> Result<Option<ImageUri>> {
        let cached = self.images_in_builder_cache().await?;
        Ok(lock_from_builder_cache(img.noscheme(), cached).map(|digest| img.lock(digest)))
    }

    /// If given an un-pinned image URI, query local image cache for its digest.
    ///
    /// Returns the given URI, along with its digest if one was found.
    ///
    /// <https://docs.docker.com/dhi/core-concepts/digests/>
    async fn maybe_lock_from_image_cache(&self, img: &ImageUri) -> Result<Option<ImageUri>> {
        if self.runner.is_none() {
            info!("Skipping inspecting image cache (runner:{})", self.runner);
            return Ok(None);
        }
        let mut cmd = self.cmd()?;
        cmd.arg("inspect").arg("--format={{index .RepoDigests 0}}").arg(img.noscheme());

        let (succeeded, stdout, stderr) = cmd.exec().await?;
        if !succeeded {
            let stderr = String::from_utf8_lossy(&stderr);
            if stderr.to_lowercase().contains("no such object") {
                return Ok(None);
            }

            let mut help = "";
            if stderr.to_lowercase().contains(" executable file not found in ")
                && self.runner_envs.contains_key(DOCKER_HOST)
            {
                // TODO: find actual solutions to 'executable file not found in $PATH'
                // error during connect: Get "http://docker.example.com/v1.51/containers/docker.io/docker/dockerfile:1/json": exec: "ssh": executable file not found in $PATH
                // error during connect: Get "http://docker.example.com/v1.51/containers/json": command [ssh -o ConnectTimeout=30 -T -- gol docker system dial-stdio] has exited with exit status 127, make sure the URL is valid, and Docker 18.09 or later is installed on the remote host: stderr=bash: line 1: docker: command not found
                help = r#"
Maybe have a look at
  https://stackoverflow.com/a/79474080/1418165
  https://github.com/docker/for-mac/issues/4382#issuecomment-603031242
"#
                .trim();
            }
            bail!("BUG: failed to inspect image cache: {stderr}{help}")
        }

        Ok(String::from_utf8_lossy(&stdout)
            .lines()
            .next()
            .and_then(|line| ImageUri::try_new(format!("docker-image://{line}")).ok())
            // NOTE: `inspect` does not keep tag: host/dir/name@sha256:digest (no :tag@)
            .map(|digested| img.lock(digested.digest())))
    }
}

/// If given an un-pinned image URI, query remote image API for its digest.
///
/// No-op for an already locked image URI.
pub(crate) async fn fetch_digest(runner: &Runner, img: &ImageUri) -> Result<ImageUri> {
    // TODO: add+impl traits on runner (fetch_digest, do_build, ..) Maybe on Green?
    if runner.is_none() {
        info!("Skipping fetching image digest (runner:{runner})");
        return Ok(img.to_owned());
    }

    if img.locked() {
        return Ok(img.to_owned());
    }

    const DOMAIN: &str = "registry.hub.docker.com";

    fn request(img: &ImageUri) -> Result<(ReqwestClient, Request)> {
        let (path, tag) = img.path_and_tag();
        let (ns, slug) = match Utf8Path::new(path).iter().collect::<Vec<_>>()[..] {
            ["docker.io", ns, slug] => (ns, slug),
            _ => bail!("BUG: unhandled registry {img:?}"),
        };

        let (client, req) = ReqwestClient::builder()
            .connect_timeout(Duration::from_secs(4))
            .build()
            .map_err(|e| anyhow!("HTTP client's config/TLS failed: {e}"))?
            .get(format!("https://{DOMAIN}/v2/repositories/{ns}/{slug}/tags/{tag}"))
            .build_split();
        let req = req.map_err(|e| {
            // e.source(): try to be a bit more helpful than just "error sending request for url"
            anyhow!("Failed to build a request against {DOMAIN}: {e} ({:?})", e.source())
        })?;
        Ok((client, req))
    }

    async fn actual(img: &ImageUri) -> Result<ImageUri> {
        let show = Once::new();
        const MAX_RETRIES: u8 = 5;
        let mut attempt = 0;
        let txt;
        loop {
            let backoff = || async move {
                let secs = 1u64 << attempt; // exponential
                warn!("hit a transient error, retrying in {secs}s ({}/{MAX_RETRIES})", attempt + 1);
                sleep(Duration::from_secs(secs)).await;
            };

            let (client, req) = request(img)?;
            show.call_once(|| {
                info!("GETing {}", req.url());
                eprintln!("GETing {}", req.url());
            });

            // Eg.: error sending request for url (https://registry.hub.docker.com/v2/repositories/moby/buildkit/tags/latest)
            let req = match client.execute(req).await {
                Ok(req) => req,
                Err(e) if attempt < MAX_RETRIES => {
                    warn!("spurious connection error: {e}");
                    backoff().await;
                    attempt += 1;
                    continue;
                }
                Err(e) => bail!("Failed to reach {DOMAIN}'s registry: {e}"),
            };

            txt = req
                .text()
                .await
                .map_err(|e| anyhow!("Failed to read response from {DOMAIN} registry: {e}"))?;
            break;
        }

        #[derive(Deserialize)]
        struct RegistryResponse {
            digest: String,
        }
        let RegistryResponse { digest } = serde_json::from_str(&txt)
            // NOTE: library images can take a few days to appear, after a Rust release:
            // Error: Failed to decode response from registry: missing field `digest` at line 1 column 130
            // {"message":"httperror 404: tag '1.89.0-slim' not found","errinfo":{"namespace":"library","repository":"rust","tag":"1.89.0-slim"}}
            .map_err(|e| anyhow!("Failed to decode response from registry: {e}\n{txt}"))?;
        // digest ~ sha256:..

        Ok(img.lock(&digest))
    }

    actual(img).await.map_err(|e| anyhow!("Failed getting digest for {img}: {e}"))
}
