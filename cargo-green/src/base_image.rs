use std::sync::LazyLock;

use anyhow::{anyhow, bail, Result};
use camino::Utf8Path;
use serde::{Deserialize, Serialize};

use crate::{
    add::Add,
    build::SHELL,
    image_uri::ImageUri,
    network::Network,
    rustup::{CHECKSUMS, VERSION},
    stage::RST,
    target_dir::replace_carefully,
    REPO,
};

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

macro_rules! ENV_COMPONENTS {
    () => {
        "CARGOGREEN_COMPONENTS"
    };
}

pub(crate) const CARGO_HOME: &str = "/usr/local/cargo";
pub(crate) const RUSTUP_HOME: &str = "/usr/local/rustup";

static BASE_FOR_RUST: LazyLock<ImageUri> = LazyLock::new(|| ImageUri::std("debian:trixie-slim"));

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

    /// https://rust-lang.github.io/rustup/environment-variables.html
    /// https://rust-lang.github.io/rustup/concepts/toolchains.html#toolchain-specification
    pub(crate) fn from_env(toolchain: &str, components: &[String], add: &Add) -> Result<Self> {
        // TODO: multiplatformify (using auto ARG.s?)
        let host = maybe_get_local_host_triple(toolchain)?;

        let Some(checksum) = CHECKSUMS.get(&host) else {
            bail!("Unhandled rustup host {host:?} please report to {REPO}")
        };

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

        let image = BASE_FOR_RUST.to_owned(); // TODO: allow overriding (CARGOGREEN_BASE_DISTRO?)
        let base = image.noscheme();
        assert!(base.contains("/debian:"));

        let (with_network, packages_block) = Add {
            // From https://github.com/rust-lang/docker-rust/blob/d14e1ad7efeb270012b1a7e88fea699b1d1082f2/nightly/alpine3.20/Dockerfile
            apk: vec!["ca-certificates".to_owned(), "gcc".to_owned()],
            // From https://github.com/rust-lang/docker-rust/blob/d14e1ad7efeb270012b1a7e88fea699b1d1082f2/nightly/bullseye/slim/Dockerfile
            apt: vec!["ca-certificates".to_owned(), "gcc".to_owned(), "libc6-dev".to_owned()],
            apt_get: vec!["ca-certificates".to_owned(), "gcc".to_owned(), "libc6-dev".to_owned()],
        }
        .union(add)
        .as_block(&format!("FROM --platform=$BUILDPLATFORM {base} AS {RST}"));

        let components = if !components.is_empty() {
            format!(" --component {}", components.join(","))
        } else {
            "".to_owned()
        };

        // Rewrite host cargo/rustc so the base_image ones can be used
        // Also, propagate RUSTUP_TOOLCHAIN so Rustup skips looking for rust-toolchain.toml
        //   If you are trying to install a package that requires a specific nightly feature or a very new stable version,
        //   you must ensure your active toolchain meets those requirements before running the install command.
        //   Cargo won't auto-switch for you based on the dependency tree.

        let block = format!(
            r#"
FROM scratch AS rustup-{toolchain}
SHELL {SHELL:?}
ADD --chmod=0144 --checksum=sha256:{checksum} \
  https://static.rust-lang.org/rustup/archive/{VERSION}/{host}/rustup-init /rustup-init
{packages_block}
ENV      RUSTUP_HOME={RUSTUP_HOME} \
    RUSTUP_TOOLCHAIN={toolchain} \
          CARGO_HOME={CARGO_HOME}
ENV CARGO=$RUSTUP_HOME/toolchains/$RUSTUP_TOOLCHAIN/bin/cargo \
    RUSTC=$RUSTUP_HOME/toolchains/$RUSTUP_TOOLCHAIN/bin/rustc \
     PATH=$CARGO_HOME/bin:$PATH
RUN \
  --mount=from=rustup-{toolchain},source=/rustup-init,dst=/rustup-init \
    set -eux \
 && /rustup-init --verbose -y --no-modify-path --profile minimal --default-toolchain {toolchain} --default-host {host}{components} \
 && chmod -R a+w $RUSTUP_HOME $CARGO_HOME
"#,
            packages_block = packages_block.trim(),
        );

        Ok(Self { with_network, image, image_inline: Some(block) })
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
        let block = self.image_inline.clone().unwrap_or_else(|| {
            let base = self.image.noscheme();
            // TODO? ARG RUST_BASE=myorg/myapp:latest \n FROM $RUST_BASE (+ similar for non-stable imgs)
            format!("FROM --platform=$BUILDPLATFORM {base} AS {RST}\n")
        });
        (self.with_network, block)
    }
}

pub(crate) fn rewrite_cargo_home(cargo_home: &Utf8Path, path: &str) -> String {
    path.replacen(CARGO_HOME, "$CARGO_HOME", 1).replacen(cargo_home.as_str(), "$CARGO_HOME", 1)
}

pub(crate) fn un_rewrite_cargo_home(txt: &str, to: &str) -> String {
    replace_carefully(txt, CARGO_HOME, to)
}

pub(crate) fn rewrite_rustup_home(val: &str) -> String {
    let val = val.replacen(RUSTUP_HOME, "$RUSTUP_HOME", 1);
    const DIR: &str = ".rustup";
    if let Some(pos) = val.find(DIR) {
        return "$RUSTUP_HOME".to_owned() + &val[(pos + DIR.len())..];
    }
    val
}

#[test]
fn test_rewrite_rustup_home() {
    assert_eq!(
        "$RUSTUP_HOME/toolchains/$RUSTUP_TOOLCHAIN/bin/rustdoc",
        rewrite_rustup_home("/home/runner/.rustup/toolchains/$RUSTUP_TOOLCHAIN/bin/rustdoc")
    );
}

fn maybe_get_local_host_triple(toolchain: &str) -> Result<String> {
    use std::str::FromStr;

    let toolchain = rustup_toolchain_manifest::Toolchain::from_str(toolchain)
        .map_err(|e| anyhow!("Failed parsing $RUSTUP_TOOLCHAIN={toolchain:?}: {e}"))?;

    if let Some(host) = toolchain.host.map(|h| h.target_triple) {
        Ok(host.to_owned())
    } else {
        rustc_host::from_cli().map_err(|e| anyhow!("Failed getting local host triple: {e}"))
    }
}

#[test]
fn test_from_env() {
    let some_stable = "1.80.0-x86_64-unknown-linux-gnu";
    let res = BaseImage::from_env(some_stable, &[], &Add::default()).unwrap();
    assert_eq!(res.image, ImageUri::std("debian:trixie-slim"));
    assert!(res.image_inline.unwrap().contains(" docker.io/library/debian:trixie-slim "));
    assert_eq!(res.with_network, Network::Default);

    let some_nightly = "nightly-2025-09-14-aarch64-apple-darwin";
    let res = BaseImage::from_env(some_nightly, &[], &Add::default()).unwrap();
    assert_eq!(res.image, ImageUri::std("debian:trixie-slim"));
    assert!(res.image_inline.unwrap().contains(" docker.io/library/debian:trixie-slim "));
    assert_eq!(res.with_network, Network::Default);
}
