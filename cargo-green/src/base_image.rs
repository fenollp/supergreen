use std::sync::LazyLock;

use anyhow::{anyhow, bail, Result};
use camino::Utf8Path;
use serde::{Deserialize, Serialize};

use crate::{
    add::Add,
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

/// Default base image: `docker-image://docker.io/library/debian:trixie-slim`
pub(crate) static BASE_IMAGE: LazyLock<ImageUri> =
    LazyLock::new(|| ImageUri::std("debian:trixie-slim"));

/// Default base image, pre-locked (on 2026-04-28)
pub(crate) static BASE_IMAGE_LOCKED: LazyLock<ImageUri> = LazyLock::new(|| {
    BASE_IMAGE.lock("sha256:cedb1ef40439206b673ee8b33a46a03a0c9fa90bf3732f54704f99cb061d2c5a")
});

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct BaseImage {
    #[doc = include_str!(concat!("../docs/",ENV_WITH_NETWORK!(),".md"))]
    #[serde(rename = "with-network")]
    pub(crate) with_network: Network,

    #[doc = include_str!(concat!("../docs/",ENV_BASE_IMAGE!(),".md"))]
    #[serde(rename = "base-image")]
    pub(crate) image: ImageUri,

    /// Computed base stage. Not user-settable.
    #[doc(hidden)]
    pub(crate) image_inline: String,
}

impl Default for BaseImage {
    fn default() -> Self {
        Self {
            with_network: Network::default(),
            image: BASE_IMAGE.clone(),
            image_inline: "".to_owned(),
        }
    }
}

impl BaseImage {
    /// https://rust-lang.github.io/rustup/environment-variables.html
    /// https://rust-lang.github.io/rustup/concepts/toolchains.html#toolchain-specification
    pub(crate) fn make_block(
        &self,
        toolchain: &str,
        components: &[String],
        add: &Add,
    ) -> Result<Self> {
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

        // TODO: find a way to install packages without requiring Network (ie using only ADDs)
        // TODO: lock distro packages we install, somehow.
        //   https://github.com/reproducible-containers/repro-sources-list.sh
        //   https://github.com/reproducible-containers/repro-pkg-cache
        //   https://github.com/reproducible-containers/repro-get

        let image = self.image.clone();

        let (with_network, packages_block) = Add {
            // From https://github.com/rust-lang/docker-rust/blob/d14e1ad7efeb270012b1a7e88fea699b1d1082f2/nightly/alpine3.20/Dockerfile
            apk: vec!["ca-certificates".to_owned(), "gcc".to_owned()],
            // From https://github.com/rust-lang/docker-rust/blob/d14e1ad7efeb270012b1a7e88fea699b1d1082f2/nightly/bullseye/slim/Dockerfile
            apt: vec!["ca-certificates".to_owned(), "gcc".to_owned(), "libc6-dev".to_owned()],
            apt_get: vec!["ca-certificates".to_owned(), "gcc".to_owned(), "libc6-dev".to_owned()],
        }
        .union(add)
        .as_block(&format!(
            "FROM --platform=$BUILDPLATFORM {base} AS {RST}",
            base = image.noscheme()
        ));

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

        let image_inline = format!(
            r#"
FROM scratch AS rustup-{toolchain}
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

        Ok(Self { with_network, image, image_inline })
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

#[cfg(test)]
#[test_case::test_matrix(["1.80.0-x86_64-unknown-linux-gnu", "nightly-2025-09-14-aarch64-apple-darwin"])]
fn base_make_block(toolchain: &str) {
    let base_image = BASE_IMAGE_LOCKED.clone();
    let base = BaseImage { image: base_image.clone(), ..Default::default() };
    assert!(base.image_inline.is_empty());
    assert_eq!(base.with_network, Network::None);

    let res = base.make_block(toolchain, &[], &Add::default()).unwrap();
    assert_eq!(res.image, base_image);
    assert!(
        res.image_inline.contains(&format!(" {} ", base_image.noscheme())),
        "In {}",
        res.image_inline
    );
    assert_eq!(res.with_network, Network::Default);
}
