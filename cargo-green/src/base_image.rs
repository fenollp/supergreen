use std::sync::LazyLock;

use rustc_version::{Channel, Version, VersionMeta};
use serde::{Deserialize, Serialize};

use crate::{add::Add, image_uri::ImageUri, network::Network, stage::RST};

macro_rules! ENV_BASE_IMAGE {
    () => {
        "CARGOGREEN_BASE_IMAGE"
    };
}

macro_rules! ENV_BASE_IMAGE_INLINE {
    () => {
        "CARGOGREEN_BASE_IMAGE_INLINE"
    };
}

macro_rules! ENV_WITH_NETWORK {
    () => {
        "CARGOGREEN_WITH_NETWORK"
    };
}

// TODO: switch to mentioning debian name: 1-slim-trixie, 1.89-slim-trixie, 1.89.0-slim-trixie, slim-trixie
// MAY help with:
//   /tmp/clis-diesel_cli_2-3-2_/release/build/proc-macro2- (required by /tmp/clis-diesel_cli_2-3-2_/release/build/proc-macro2-3093cf4d56979071/build-script-build)

static STABLE_RUST: LazyLock<ImageUri> = LazyLock::new(|| ImageUri::std("rust:1-slim"));
static BASE_FOR_RUST: LazyLock<ImageUri> = LazyLock::new(|| ImageUri::std("debian:stable-slim"));

#[test]
fn default_is_unset() {
    assert!(BaseImage::default().is_unset());
}

#[derive(Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct BaseImage {
    #[doc = include_str!(concat!("../docs/",ENV_WITH_NETWORK!(),".md"))]
    #[serde(rename = "with-network")]
    pub(crate) with_network: Network,

    #[doc = include_str!(concat!("../docs/",ENV_BASE_IMAGE!(),".md"))]
    #[serde(rename = "base-image")]
    pub(crate) image: ImageUri,

    #[doc = include_str!(concat!("../docs/",ENV_BASE_IMAGE_INLINE!(),".md"))]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "base-image-inline")]
    pub(crate) image_inline: Option<String>,
}

impl BaseImage {
    #[must_use]
    pub(crate) fn from_image(image: ImageUri) -> Self {
        Self { image, ..Default::default() }
    }

    #[must_use]
    pub(crate) fn is_unset(&self) -> bool {
        self.image.is_empty() && self.image_inline.is_none()
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
        VersionMeta { semver, commit_hash, commit_date, channel, host, .. }: VersionMeta,
    ) -> Option<Self> {
        if channel == Channel::Stable {
            assert!(STABLE_RUST.contains(":1-"));
            let minored = STABLE_RUST.as_str().replace(":1-", &format!(":{semver}-"));
            return Some(Self::from_image(minored.try_into().unwrap()));
        }
        commit_hash.zip(commit_date).map(|(commit, date)| {
            RustcV { version: semver, commit, date, channel, host }.as_base_image()
        })
    }

    #[must_use]
    pub(crate) fn lock_base_to(self, image: ImageUri) -> Self {
        let image_inline = self.image_inline.map(|block| {
            let from = self.image.noscheme();
            let to = image.noscheme();
            block.replace(&format!(" {from} "), &format!(" {to} "))
        });
        Self { image, image_inline, ..self }
    }

    #[must_use]
    pub(crate) fn as_block(&self) -> (Network, String) {
        let mut with_network = self.with_network;
        let block = self.image_inline.clone().unwrap_or_else(|| {
            let base = self.image.noscheme();
            // TODO? ARG RUST_BASE=myorg/myapp:latest \n FROM $RUST_BASE (+ similar for non-stable imgs)
            let target = "aarch64-apple-darwin";
            let target_is_host = false;
            let add = if target_is_host {
                "".to_owned()
            } else {
                with_network = Network::Default;
                format!("\nRUN rustup target add {target}")
            };
            format!("FROM --platform=$BUILDPLATFORM {base} AS {RST}{add}\n")
        });
        (with_network, block)
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
    host: String,
}

impl RustcV {
    #[must_use]
    fn as_base_image(&self) -> BaseImage {
        // TODO: dynamically resolve + cache this, if network is up.
        //TODO: hardcode per host + script to check + bump and release when new ones
        let rustup_version = "1.28.1";
        let rustup_checksum = "a3339fb004c3d0bb9862ba0bce001861fe5cbde9c10d16591eb3f39ee6cd3e7f";

        // FIXME: multiplatformify (using auto ARG.s) (use rustc_version::VersionMeta.host)
        // let host = "x86_64-unknown-linux-gnu";
        // let host = "aarch64-apple-darwin";
        let host = &self.host;

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

        let image = BASE_FOR_RUST.to_owned();
        let base = image.noscheme();
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
FROM scratch AS rustup-{short_checksum}
SHELL
ADD --link --chmod=0144 --checksum=sha256:{rustup_checksum} \
  https://static.rust-lang.org/rustup/archive/{rustup_version}/{host}/rustup-init /rustup-init
{packages_block}
ENV RUSTUP_HOME=/usr/local/rustup \
     CARGO_HOME=/usr/local/cargo \
           PATH=/usr/local/cargo/bin:$PATH
RUN \
 --mount=from=rustup-{short_checksum},source=/rustup-init,dst=/rustup-init \
   set -eux \ <<EOR
&& /rustup-init --verbose -y --no-modify-path --profile minimal --default-toolchain {channel}-{date} --default-host {host} --target {host} \
&& chmod -R a+w $RUSTUP_HOME $CARGO_HOME \
&& rustup --version \
&& cargo --version \
&& rustc --version
"#,
            packages_block = packages_block.trim(),
            short_checksum = &rustup_checksum[..7],
        );

        BaseImage { with_network, image, image_inline: Some(block) }
    }
}

// From https://github.com/rust-lang/rustup/blob/2d80024a0fe21bd9f082d89f672a471ef638562e/rustup-init.sh#L57
//
// rustup-init 1.29.0 (2eaaa4bb0 2025-12-11)
//
// The installer for rustup
//
// Usage: rustup-init[EXE] [OPTIONS]
//
// Options:
//   -v, --verbose
//           Set log level to 'DEBUG' if 'RUSTUP_LOG' is unset
//   -q, --quiet
//           Disable progress output, set log level to 'WARN' if 'RUSTUP_LOG' is unset
//   -y
//           Disable confirmation prompt
//       --default-host <DEFAULT_HOST>
//           Choose a default host triple
//       --default-toolchain <DEFAULT_TOOLCHAIN>
//           Choose a default toolchain to install. Use 'none' to not install any toolchains at all
//       --profile <PROFILE>
//           [default: default] [possible values: minimal, default, complete]
//   -c, --component <COMPONENT>
//           Comma-separated list of component names to also install
//   -t, --target <TARGET>
//           Comma-separated list of target names to also install
//       --no-update-default-toolchain
//           Don't update any existing default toolchain after install
//       --no-modify-path
//           Don't configure the PATH environment variable
//   -h, --help
//           Print help
//   -V, --version
//           Print version

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
    assert_eq!(res.image, ImageUri::std("rust:1.80.0-slim"));
    assert_eq!(res.image_inline, None);
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
    assert_eq!(res.image, ImageUri::std("debian:stable-slim"));
    assert!(res.image_inline.unwrap().contains(" docker.io/library/debian:stable-slim "));
    assert_eq!(res.with_network, Network::Default);
}
