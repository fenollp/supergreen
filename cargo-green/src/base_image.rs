use std::sync::LazyLock;

use rustc_version::{Channel, Version, VersionMeta};
use serde::{Deserialize, Serialize};

use crate::{add::Add, image_uri::ImageUri, runner::Network, stage::RST};

// Envs that override Cargo.toml settings
pub(crate) const ENV_BASE_IMAGE: &str = "CARGOGREEN_BASE_IMAGE";
pub(crate) const ENV_BASE_IMAGE_INLINE: &str = "CARGOGREEN_BASE_IMAGE_INLINE";
pub(crate) const ENV_WITH_NETWORK: &str = "CARGOGREEN_WITH_NETWORK";

static STABLE_RUST: LazyLock<ImageUri> = LazyLock::new(|| ImageUri::std("rust:1-slim"));
static BASE_FOR_RUST: LazyLock<ImageUri> = LazyLock::new(|| ImageUri::std("debian:stable-slim"));

#[test]
fn default_is_unset() {
    assert!(BaseImage::default().is_unset());
}

#[derive(Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct BaseImage {
    // Controls runner's `--network none (default) | default | host` setting.
    //
    // Set this to "default" if e.g. your `base-image-inline` calls curl or wget or installs some packages.
    //
    // # This environment variable takes precedence over any Cargo.toml settings:
    // CARGOGREEN_WITH_NETWORK="none"
    //
    // Set to `none` when in $CARGO_NET_OFFLINE mode. See
    //   * https://doc.rust-lang.org/cargo/reference/config.html#netoffline
    //   * https://github.com/rust-lang/rustup/issues/4289
    pub(crate) with_network: Network,

    // Sets the base Rust image, as an image URL (or any build context, actually).
    //
    // If needing additional envs to be passed to rustc or build script, set them in the base image.
    // This can be done in that same config file with `base-image-inline`.
    //
    // See also:
    //   `also-run`
    //   `base-image-inline`
    //   `additional-build-arguments`
    // For remote builds: make sure this is accessible non-locally.
    //
    // base-image = "docker-image://docker.io/library/rust:1-slim"
    //
    // The value must start with docker-image:// and image must be available on the DOCKER_HOST, eg:
    // CARGOGREEN_BASE_IMAGE=docker-image://rustc_with_libs
    // DOCKER_HOST=ssh://my-remote-builder docker buildx build -t rustc_with_libs - <<EOF
    // FROM docker.io/library/rust:1.69.0-slim-bookworm@sha256:8bdd28ef184d85c9b4932586af6280732780e806e5f452065420f2e783323ca3
    // RUN set -eux && apt update && apt install -y libpq-dev libssl3
    // ENV KEY=value
    // EOF
    //
    // # This environment variable takes precedence over any Cargo.toml settings:
    // CARGOGREEN_BASE_IMAGE="docker-image://docker.io/library/rust:1-slim"
    pub(crate) base_image: ImageUri,

    // Sets the base Rust image for root package and all dependencies, unless themselves being configured differently.
    // See also:
    //   `with-network`
    //   `additional-build-arguments`
    //
    // In order to avoid unexpected changes, you may want to pin the image using an immutable digest.
    // Note that carefully crafting crossplatform stages can be non-trivial.
    //
    // base-image-inline = """
    // FROM --platform=$BUILDPLATFORM rust:1 AS rust-base
    // RUN --mount=from=some-context,dst=/tmp/some-context cp -r /tmp/some-context ./
    // RUN --mount=type=secret,id=aws
    // """
    // base-image = "docker-image://rust:1" # This must also be set so digest gets pinned automatically.
    //
    // # This environment variable takes precedence over any Cargo.toml settings:
    // CARGOGREEN_BASE_IMAGE="FROM=rust:1 AS rust-base\nRUN --mount=from=some-context,dst=/tmp/some-context cp -r /tmp/some-context ./\nRUN --mount=type=secret,id=aws\n"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) base_image_inline: Option<String>,
}

impl BaseImage {
    #[must_use]
    pub(crate) fn from_image(base_image: ImageUri) -> Self {
        Self { base_image, ..Default::default() }
    }

    #[must_use]
    pub(crate) fn is_unset(&self) -> bool {
        self.base_image.is_empty() && self.base_image_inline.is_none()
    }

    #[must_use]
    pub(crate) fn from_local_rustc() -> Self {
        rustc_version::version_meta()
            .ok()
            .and_then(Self::from_rustcv)
            .unwrap_or_else(|| Self::from_image(STABLE_RUST.to_owned()))
    }

    #[must_use]
    fn from_rustcv(
        VersionMeta { semver, commit_hash, commit_date, channel, .. }: VersionMeta,
    ) -> Option<Self> {
        if channel == Channel::Stable {
            assert!(STABLE_RUST.contains(":1-"));
            let minored = STABLE_RUST.as_str().replace(":1-", &format!(":{semver}-"));
            return Some(Self::from_image(minored.try_into().unwrap()));
        }
        commit_hash
            .zip(commit_date)
            .map(|(commit, date)| RustcV { version: semver, commit, date, channel }.as_base_image())
    }

    #[must_use]
    pub(crate) fn lock_base_to(self, base_image: ImageUri) -> Self {
        let base_image_inline = self.base_image_inline.map(|block| {
            let from = self.base_image.noscheme();
            let to = base_image.noscheme();
            block.replace(&format!(" {from} "), &format!(" {to} "))
        });
        Self { base_image, base_image_inline, ..self }
    }

    #[must_use]
    pub(crate) fn as_block(&self) -> (Network, String) {
        let block = self.base_image_inline.clone().unwrap_or_else(|| {
            let base = self.base_image.noscheme();
            // TODO? ARG RUST_BASE=myorg/myapp:latest \n FROM $RUST_BASE (+ similar for non-stable imgs)
            format!("FROM --platform=$BUILDPLATFORM {base} AS {RST}\n")
        });
        (self.with_network, block)
    }
}

// TODO? maybe use commit & version as selector too?
struct RustcV {
    #[expect(unused)]
    version: Version,
    #[expect(unused)]
    commit: String,
    date: String,
    channel: Channel,
}

impl RustcV {
    #[must_use]
    fn as_base_image(&self) -> BaseImage {
        // TODO: dynamically resolve + cache this, if network is up.
        let rustup_version = "1.28.1";
        let rustup_checksum = "a3339fb004c3d0bb9862ba0bce001861fe5cbde9c10d16591eb3f39ee6cd3e7f";

        // FIXME: multiplatformify (using auto ARG.s) (use rustc_version::VersionMeta.host)
        let host = "x86_64-unknown-linux-gnu";

        // have buildkit call rustc with `--target $(adapted $TARGETPLATFORM)`, if not given `--target`
        // `adapted` translates buildkit platform format to rustc's
        //
        // maybe that's too naive
        //   do more research with `cargo cross`
        //
        // Use https://github.com/search?q=repo%3Across-rs/cross%20path%3Adockerfile&type=code images as auto base image?
        //
        // osx https://github.com/tonistiigi/xx?tab=readme-ov-file#external-sdk-support
        //
        // https://github.com/tonistiigi/xx?tab=readme-ov-file#rust
        // xx-cargo
        //
        // RUN apk add clang lld
        // ARG TARGETPLATFORM
        // RUN cargo build --target=$(xx-cargo --print-target-triple) --release --target-dir ./build && \
        //     xx-verify ./build/$(xx-cargo --print-target-triple)/release/hello_cargo

        // TODO: lock distro packages we install, somehow.
        //   https://github.com/reproducible-containers/repro-sources-list.sh
        //   https://github.com/reproducible-containers/repro-pkg-cache
        //   https://github.com/reproducible-containers/repro-get

        let RustcV { date, channel, .. } = self;
        let channel = match channel {
            Channel::Stable => "stable",
            Channel::Dev => "dev",
            Channel::Beta => "beta",
            Channel::Nightly => "nightly",
        };
        //=> sub fn that takes "{channel}-{date}" in, because rustup takes somewhat-freeform toolchain specs
        //==> $RUSTUP_TOOLCHAIN https://rust-lang.github.io/rustup/environment-variables.html

        let base_image = BASE_FOR_RUST.to_owned();
        let base = base_image.noscheme();
        assert!(base.contains("/debian:"));

        let (with_network, packages_block) = Add {
            // From https://github.com/rust-lang/docker-rust/blob/d14e1ad7efeb270012b1a7e88fea699b1d1082f2/nightly/alpine3.20/Dockerfile
            apk: vec!["ca-certificates".to_owned(), "gcc".to_owned()],
            // From https://github.com/rust-lang/docker-rust/blob/d14e1ad7efeb270012b1a7e88fea699b1d1082f2/nightly/bullseye/slim/Dockerfile
            apt: vec!["ca-certificates".to_owned(), "gcc".to_owned(), "libc6-dev".to_owned()],
            apt_get: vec!["ca-certificates".to_owned(), "gcc".to_owned(), "libc6-dev".to_owned()],
        }
        .as_block(&format!("FROM --platform=$BUILDPLATFORM {base} AS {RST}"));

        let block = format!(
            r#"
FROM scratch AS rustup-{channel}-{date}
ADD --chmod=0144 --checksum=sha256:{rustup_checksum} \
  https://static.rust-lang.org/rustup/archive/{rustup_version}/{host}/rustup-init /rustup-init
{packages_block}
ENV RUSTUP_HOME=/usr/local/rustup \
     CARGO_HOME=/usr/local/cargo \
           PATH=/usr/local/cargo/bin:$PATH
RUN \
 --mount=from=rustup-{channel}-{date},source=/rustup-init,dst=/rustup-init \
   set -eux \
&& /rustup-init --verbose -y --no-modify-path --profile minimal --default-toolchain {channel}-{date} --default-host {host} \
&& chmod -R a+w $RUSTUP_HOME $CARGO_HOME \
&& rustup --version \
&& cargo --version \
&& rustc --version
"#,
            packages_block = packages_block.trim(),
        );

        BaseImage { with_network, base_image, base_image_inline: Some(block) }
    }
}

#[test]
fn test_from_rustc_v() {
    use rustc_version::version_meta_for;

    let some_stable = version_meta_for(
        &r#"
rustc 1.80.0 (051478957 2024-07-21)
binary: rustc
commit-hash: 051478957371ee0084a7c0913941d2a8c4757bb9
commit-date: 2024-07-21
host: x86_64-unknown-linux-gnu
release: 1.80.0
LLVM version: 18.1.7
"#[1..],
    )
    .unwrap();
    let res = BaseImage::from_rustcv(some_stable).unwrap();
    assert_eq!(res.base_image, ImageUri::std("rust:1.80.0-slim"));
    assert_eq!(res.base_image_inline, None);
    assert_eq!(res.with_network, Network::None);

    let some_nightly = version_meta_for(
        &r#"
rustc 1.82.0-nightly (60d146580 2024-08-06)
binary: rustc
commit-hash: 60d146580c10036ce89e019422c6bc2fd9729b65
commit-date: 2024-08-06
host: x86_64-unknown-linux-gnu
release: 1.82.0-nightly
LLVM version: 19.1.0
"#[1..],
    )
    .unwrap();
    let res = BaseImage::from_rustcv(some_nightly).unwrap();
    assert_eq!(res.base_image, ImageUri::std("debian:stable-slim"));
    assert!(res.base_image_inline.unwrap().contains(" docker.io/library/debian:stable-slim "));
    assert_eq!(res.with_network, Network::Default);
}
