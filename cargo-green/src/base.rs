use rustc_version::{Channel, Version, VersionMeta};
use serde::{Deserialize, Serialize};

pub(crate) const RUST: &str = "rust-base";

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(default)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct BaseImage {
    // Sets the base Rust image, as an image URL.
    // See also:
    //   `also-run`
    //   `base-image-inline`
    //   `additional-build-arguments`
    // For remote builds: make sure this is accessible non-locally.
    //
    // base-image = "docker-image://docker.io/library/rust:1-slim"
    //
    // # This environment variable takes precedence over any Cargo.toml settings:
    // CARGOGREEN_BASE_IMAGE="docker-image://docker.io/library/rust:1-slim"
    //
    //TODO: rework this: v
    // A Docker image or any build context, actually.
    // If needing additional envs to be passed to rustc or buildrs, set them in the base image.
    // CARGOGREEN_BASE_IMAGE must start with docker-image:// and image MUST be available on DOCKER_HOST e.g.:
    // CARGOGREEN_BASE_IMAGE=docker-image://rustc_with_libs
    // DOCKER_HOST=ssh://oomphy docker buildx build -t rustc_with_libs - <<EOF
    // FROM docker.io/library/rust:1.69.0-slim-bookworm@sha256:8bdd28ef184d85c9b4932586af6280732780e806e5f452065420f2e783323ca3
    // RUN set -eux && apt update && apt install -y libpq-dev libssl3
    // EOF
    pub(crate) base_image: String, //TODO? url? docker-..://...

    // Sets the base Rust image for root package and all dependencies, unless themselves being configured differently.
    // See also:
    //   `additional-build-arguments`
    // In order to avoid unexpected changes, you may want to pin the image using an immutable digest.
    // Note that carefully crafting crossplatform stages can be non-trivial.
    //
    // base-image-inline = """
    // FROM --platform=$BUILDPLATFORM rust:1 AS rust-base
    // RUN --mount=from=some-context,target=/tmp/some-context cp -r /tmp/some-context ./
    // RUN --mount=type=secret,id=aws
    // """
    //
    // # This environment variable takes precedence over any Cargo.toml settings:
    // CARGOGREEN_BASE_IMAGE="FROM=rust:1 AS rust-base\nRUN --mount=from=some-context,target=/tmp/some-context cp -r /tmp/some-context ./\nRUN --mount=type=secret,id=aws\n"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) base_image_inline: Option<String>,

    #[serde(skip)]
    with_network: bool, //FIXME: CARGOGREEN_NETWORK
}

#[test]
fn default_is_unset() {
    assert!(BaseImage::default().is_unset());
}

const STABLE_RUST: &str = "docker-image://docker.io/library/rust:1-slim";

impl BaseImage {
    #[must_use]
    pub(crate) fn from_image(base_image: String) -> Self {
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
            return Some(Self::from_image(STABLE_RUST.replace(":1-", &format!(":{semver}-"))));
        }
        const BASE_FOR_RUST: &str = "docker-image://docker.io/library/debian:stable-slim";
        commit_hash.zip(commit_date).map(|(commit, date)| Self {
            base_image: BASE_FOR_RUST.to_owned(),
            with_network: true,
            base_image_inline: Some(
                RustcV { version: semver, commit, date, channel }.to_inline(BASE_FOR_RUST),
            ),
        })
    }

    #[must_use]
    pub(crate) fn lock_base_to(self, base_image: String) -> Self {
        let base_image_inline = self.base_image_inline.map(|inline| {
            let from = self.base_image.trim_start_matches("docker-image://");
            let to = base_image.trim_start_matches("docker-image://");
            inline.replace(&format!(" {from} "), &format!(" {to} "))
        });
        Self { base_image, base_image_inline, ..self }
    }

    #[must_use]
    pub(crate) fn block(&self) -> (String, bool) {
        let inline = self.base_image_inline.clone().unwrap_or_else(|| {
            let base = self.base_image.trim_start_matches("docker-image://");
            format!("FROM --platform=$BUILDPLATFORM {base} AS {RUST}\n")
        });
        (inline, self.with_network)
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
    fn to_inline(&self, base: &str) -> String {
        let base = base.trim_start_matches("docker-image://");

        // FIXME: multiplatformify (using auto ARG.s) (use rustc_version::VersionMeta.host)

        let RustcV { date, channel, .. } = self;

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

        // TODO: use https://github.com/reproducible-containers/repro-sources-list.sh

        let channel = match channel {
            Channel::Stable => "stable",
            Channel::Dev => "dev",
            Channel::Beta => "beta",
            Channel::Nightly => "nightly",
        };

        // Inspired from https://github.com/rust-lang/docker-rust/blob/d14e1ad7efeb270012b1a7e88fea699b1d1082f2/nightly/bullseye/slim/Dockerfile
        format!(
            r#"
FROM scratch AS rustup
ADD --chmod=0755 --checksum=sha256:6aeece6993e902708983b209d04c0d1dbb14ebb405ddb87def578d41f920f56d \
  https://static.rust-lang.org/rustup/archive/1.27.1/x86_64-unknown-linux-gnu/rustup-init /rustup-init
FROM --platform=$BUILDPLATFORM {base} AS {RUST}
ENV RUSTUP_HOME=/usr/local/rustup \
     CARGO_HOME=/usr/local/cargo \
           PATH=/usr/local/cargo/bin:$PATH
RUN \
    set -ux \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates \
      gcc \
      libc6-dev
RUN \
  --mount=from=rustup,source=/rustup-init,target=/rustup-init \
    set -ux \
 && /rustup-init -y --no-modify-path --profile minimal --default-toolchain {channel}-{date} --default-host x86_64-unknown-linux-gnu \
 && chmod -R a+w $RUSTUP_HOME $CARGO_HOME \
 && rustup --version \
 && cargo --version \
 && rustc --version \
 && apt-get remove -y --auto-remove \
 && rm -rf /var/lib/apt/lists/* \
# clean up for reproducibility
 && rm -rf /var/log/* /var/cache/ldconfig/aux-cache
"#
        )
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
    assert_eq!(res.base_image, "docker-image://docker.io/library/rust:1.80.0-slim");
    assert_eq!(res.base_image_inline, None);
    assert!(!res.with_network);

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
    assert_eq!(res.base_image, "docker-image://docker.io/library/debian:stable-slim");
    assert!(res.base_image_inline.unwrap().contains(" docker.io/library/debian:stable-slim "));
    assert!(res.with_network);
}
