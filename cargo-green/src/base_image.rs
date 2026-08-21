use std::{fs, io::ErrorKind, sync::LazyLock};

use anyhow::{Result, anyhow, bail};
use camino::Utf8Path;
use serde::{Deserialize, Serialize};

use crate::{
    REPO,
    add::Add,
    all_our_envs::RUSTUP_TOOLCHAIN,
    dirs::{Paths, replace_tokens},
    image_uri::ImageUri,
    network::Network,
    rustup::{CHECKSUMS, VERSION},
    stage::RST,
};

const CARGO_HOME: &str = "/usr/local/cargo";
const RUSTUP_HOME: &str = "/usr/local/rustup";

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
    #[doc = envdocs!(CARGOGREEN_WITH_NETWORK)]
    #[serde(rename = "with-network")]
    pub(crate) with_network: Network,

    #[doc = envdocs!(CARGOGREEN_BASE_IMAGE)]
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
    /// <https://rust-lang.github.io/rustup/environment-variables.html>
    /// <https://rust-lang.github.io/rustup/concepts/toolchains.html#toolchain-specification>
    pub(crate) fn make_block(
        &self,
        toolchain: &str,
        components: &[String],
        target: Option<&str>,
        add: &Add,
    ) -> Result<Self> {
        // TODO: multiplatformify (using auto ARG.s?)
        let host = maybe_get_local_host_triple(toolchain)?;

        let Some(checksum) = CHECKSUMS.get(&host) else {
            bail!("Unhandled rustup host {host:?} please report to {REPO}")
        };

        let image = self.image.clone();

        let components = if !components.is_empty() {
            format!(" --component {}", components.join(","))
        } else {
            "".to_owned()
        };

        // https://scribe.rip/com/better-programming/cross-compiling-rust-from-mac-to-linux-7fad5a454ab1
        // https://github.com/rust-cross/cargo-zigbuild/blob/f36f2f23c169937b680a963595e5002cf79f1cc8/src/zig.rs#L879
        // https://github.com/cross-rs/cross/pkgs/container/armv7-unknown-linux-musleabihf/68145882?tag=0.2.5
        // https://github.com/cross-rs/cross/blob/v0.2.5/docker/Dockerfile.armv7-unknown-linux-musleabihf
        // https://github.com/cross-rs/cross/wiki/Contributing#how-cross-works

        let target = target.map(|target| format!(" --target {target}")).unwrap_or_default();

        // Rewrite host cargo/rustc so the base_image ones can be used
        // Also, propagate RUSTUP_TOOLCHAIN so Rustup skips looking for rust-toolchain.toml
        //   If you are trying to install a package that requires a specific nightly feature or a very new stable version,
        //   you must ensure your active toolchain meets those requirements before running the install command.
        //   Cargo won't auto-switch for you based on the dependency tree.

        let rustup_block = format!(
            r#"
FROM scratch AS rustup-{toolchain}
ADD --chmod=u+x --checksum=sha256:{checksum} \
  https://static.rust-lang.org/rustup/archive/{VERSION}/{host}/rustup-init /rustup-init
FROM --platform=$BUILDPLATFORM {base} AS {RST}
SHELL {shell:?}
ENV       CARGO_HOME={CARGO_HOME} \
         RUSTUP_HOME={RUSTUP_HOME} \
    RUSTUP_TOOLCHAIN={toolchain}
ENV CARGO=$RUSTUP_HOME/toolchains/{RUSTUP_TOOLCHAIN}/bin/cargo \
    RUSTC=$RUSTUP_HOME/toolchains/{RUSTUP_TOOLCHAIN}/bin/rustc \
     PATH=$CARGO_HOME/bin:$PATH
RUN \
  --mount=from=rustup-{toolchain},source=/rustup-init,dst=/rustup-init \
    set -eux \
 && /rustup-init --verbose -y --no-modify-path --profile minimal --default-toolchain {toolchain} --default-host {host}{target}{components} \
 && chmod -R a+w $RUSTUP_HOME $CARGO_HOME
"#,
            shell = ["/bin/sh", "-eux", "-c"],
            base = image.noscheme(),
        );
        let with_network = Network::Default; // rustup-init requires network TODO: turn rustup-init calls into ADDs

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

        let (_with_network, image_inline) = Add {
            // From https://github.com/rust-lang/docker-rust/blob/d14e1ad7efeb270012b1a7e88fea699b1d1082f2/nightly/alpine3.20/Dockerfile
            apk: vec!["ca-certificates".to_owned(), "gcc".to_owned()],
            // From https://github.com/rust-lang/docker-rust/blob/d14e1ad7efeb270012b1a7e88fea699b1d1082f2/nightly/bullseye/slim/Dockerfile
            apt: vec!["ca-certificates".to_owned(), "gcc".to_owned(), "libc6-dev".to_owned()],
        }
        .union(add)
        .as_block(&rustup_block);

        Ok(Self { with_network, image, image_inline })
    }
}

impl Paths {
    pub(crate) fn rewrite_cargo_home(&self, path: &str) -> String {
        path.replacen(CARGO_HOME, "$CARGO_HOME", 1).replacen(
            self.cargo_home.as_str(),
            "$CARGO_HOME",
            1,
        )
    }

    pub(crate) fn un_rewrite_cargo_home(&self, txt: &str) -> String {
        replace_tokens(txt, CARGO_HOME, self.cargo_home.as_str(), false)
    }
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
    use crate::all_our_envs::RUSTUP_TOOLCHAIN;
    assert_eq!(
        format!("$RUSTUP_HOME/toolchains/{RUSTUP_TOOLCHAIN}/bin/rustdoc"),
        rewrite_rustup_home(&format!(
            "/home/runner/.rustup/toolchains/{RUSTUP_TOOLCHAIN}/bin/rustdoc"
        ))
    );
}

fn maybe_get_local_host_triple(toolchain: &str) -> Result<String> {
    use std::str::FromStr;

    let toolchain = rustup_toolchain_manifest::Toolchain::from_str(toolchain)
        .map_err(|e| anyhow!("Failed parsing {RUSTUP_TOOLCHAIN}={toolchain:?}: {e}"))?;

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

    let res = base.make_block(toolchain, &[], None, &Add::default()).unwrap();
    assert_eq!(res.image, base_image);
    assert!(
        res.image_inline.contains(&format!(" {} ", base_image.noscheme())),
        "In {}",
        res.image_inline
    );
    assert_eq!(res.with_network, Network::Default);
}

impl Paths {
    pub(crate) fn setup(&self) -> Result<()> {
        let _ = fs::create_dir_all(&self.cargo_home);
        let usage = "{ cargo green supergreen setup 2>/dev/null || true; } | sudo /bin/sh -xe";

        let (guest, host) = (Utf8Path::new(CARGO_HOME), &self.cargo_home);
        if !guest.exists() {
            eprintln!("Execute the following commands, or pipe them with: `{usage}`");
            eprintln!();
            let cmd = format!("ln -s {host} {guest}");
            println!("{cmd}");
            eprintln!();
            if let Err(e) = symlink::symlink_dir(host, guest)
                && e.kind() != ErrorKind::AlreadyExists
            {
                bail!(
                    "Trying to ensure guest $CARGO_HOME is followable from host, but:
Could not `{cmd}`:
    {e}

Please try:
    {usage}
"
                )
            }
        }

        self.maybe_arrange_cratesio_index()?;
        Ok(())
    }
}

/// The `rust-base` stage every crate's Containerfile starts from: a pinned `rustup-init`
/// fetched by digest, then a toolchain installed into a fixed `$CARGO_HOME`/`$RUSTUP_HOME`
/// so nothing about the host leaks into the layer.
#[cfg(test)]
mod block {
    use snapbox::str;

    use super::{Add, BASE_IMAGE_LOCKED, BaseImage, Network};
    use crate::containerfile::assert_containerfile_eq;

    const STABLE: &str = "1.94.0-x86_64-unknown-linux-gnu";

    fn block(components: &[&str], target: Option<&str>, add: Add) -> String {
        let base = BaseImage { image: BASE_IMAGE_LOCKED.clone(), ..Default::default() };
        let components: Vec<_> = components.iter().map(ToString::to_string).collect();
        base.make_block(STABLE, &components, target, &add).unwrap().image_inline
    }

    #[test]
    fn the_default_toolchain_stage() {
        assert_containerfile_eq!(
            block(&[], None, Add::default()),
            str![[r#"

FROM --platform=$BUILDPLATFORM docker.io/tonistiigi/xx:1.6.1@sha256:923441d7c25f1e2eb5789f82d987693c47b8ed987c4ab3b075d6ed2b5d6779a3 AS xx
FROM scratch AS rustup-1.94.0-x86_64-unknown-linux-gnu
ADD --chmod=u+x --checksum=sha256:4acc9acc76d5079515b46346a485974457b5a79893cfb01112423c89aeb5aa10 \
  https://static.rust-lang.org/rustup/archive/1.29.0/x86_64-unknown-linux-gnu/rustup-init /rustup-init
FROM --platform=$BUILDPLATFORM docker.io/library/debian:trixie-slim@sha256:cedb1ef40439206b673ee8b33a46a03a0c9fa90bf3732f54704f99cb061d2c5a AS rust-base
SHELL ["/bin/sh", "-eux", "-c"]
ENV       CARGO_HOME=/usr/local/cargo \
         RUSTUP_HOME=/usr/local/rustup \
    RUSTUP_TOOLCHAIN=1.94.0-x86_64-unknown-linux-gnu
ENV CARGO=$RUSTUP_HOME/toolchains/$RUSTUP_TOOLCHAIN/bin/cargo \
    RUSTC=$RUSTUP_HOME/toolchains/$RUSTUP_TOOLCHAIN/bin/rustc \
     PATH=$CARGO_HOME/bin:$PATH
RUN \
  --mount=from=rustup-1.94.0-x86_64-unknown-linux-gnu,source=/rustup-init,dst=/rustup-init \
    set -eux \
 && /rustup-init --verbose -y --no-modify-path --profile minimal --default-toolchain 1.94.0-x86_64-unknown-linux-gnu --default-host x86_64-unknown-linux-gnu \
 && chmod -R a+w $RUSTUP_HOME $CARGO_HOME
ARG TARGETPLATFORM
RUN \
  --mount=from=xx,source=/usr/bin/xx-apk,dst=/usr/bin/xx-apk \
  --mount=from=xx,source=/usr/bin/xx-apt,dst=/usr/bin/xx-apt-get \
  --mount=from=xx,source=/usr/bin/xx-cc,dst=/usr/bin/xx-c++ \
  --mount=from=xx,source=/usr/bin/xx-cargo,dst=/usr/bin/xx-cargo \
  --mount=from=xx,source=/usr/bin/xx-cc,dst=/usr/bin/xx-cc \
  --mount=from=xx,source=/usr/bin/xx-cc,dst=/usr/bin/xx-clang \
  --mount=from=xx,source=/usr/bin/xx-cc,dst=/usr/bin/xx-clang++ \
  --mount=from=xx,source=/usr/bin/xx-go,dst=/usr/bin/xx-go \
  --mount=from=xx,source=/usr/bin/xx-info,dst=/usr/bin/xx-info \
  --mount=from=xx,source=/usr/bin/xx-ld-shas,dst=/usr/bin/xx-ld-shas \
  --mount=from=xx,source=/usr/bin/xx-verify,dst=/usr/bin/xx-verify \
  --mount=from=xx,source=/usr/bin/xx-windres,dst=/usr/bin/xx-windres \
    set -eux \
 && if command -v apk >/dev/null 2>&1; then \
                                                          xx-apk     add     --no-cache                 'ca-certificates' 'gcc'; \
    else \
      xx-apt-get update && DEBIAN_FRONTEND=noninteractive xx-apt-get satisfy --no-install-recommends -y 'ca-certificates' 'gcc' 'libc6-dev'; \
    fi

"#]]
        );
    }

    /// `--component` and `--target` are appended to the same `rustup-init` call, so a
    /// cross-compiling build still installs exactly one toolchain.
    #[test]
    fn components_and_a_cross_target() {
        let block =
            block(&["clippy", "rustfmt"], Some("armv7-unknown-linux-musleabihf"), Add::default());
        assert!(
            block.contains(
                " --default-host x86_64-unknown-linux-gnu --target armv7-unknown-linux-musleabihf --component clippy,rustfmt \\\n"
            ),
            "in {block}"
        );
    }

    /// A user's `add` extends the baseline rather than replacing it.
    #[test]
    fn extra_packages_extend_the_baseline() {
        let base = BaseImage { image: BASE_IMAGE_LOCKED.clone(), ..Default::default() };
        let add = Add { apt: vec!["libssl-dev".to_owned()], apk: vec!["openssl-dev".to_owned()] };
        let made = base.make_block(STABLE, &[], None, &add).unwrap();

        // rustup-init always needs the network; packages do not change that verdict.
        assert_eq!(made.with_network, Network::Default);

        let block = made.image_inline;
        assert!(block.contains("AS xx\n"), "in {block}");
        // Defaults are merged in, sorted and deduped, not replaced.
        assert!(block.contains("'ca-certificates' 'gcc' 'openssl-dev'"), "in {block}");
        assert!(block.contains("'ca-certificates' 'gcc' 'libc6-dev' 'libssl-dev'"), "in {block}");
    }

    /// Even with an empty `add`, `make_block` unions in a hardcoded baseline
    /// (`ca-certificates`, `gcc`, `libc6-dev`), so every base stage installs packages
    /// and therefore always carries the `xx` helpers and needs the network.
    #[test]
    fn the_baseline_packages_are_never_optional() {
        let base = BaseImage { image: BASE_IMAGE_LOCKED.clone(), ..Default::default() };
        let made = base.make_block(STABLE, &[], None, &Add::default()).unwrap();

        assert_eq!(made.with_network, Network::Default);
        assert!(made.image_inline.contains("AS xx\n"), "in {}", made.image_inline);
        assert!(
            made.image_inline.contains("'ca-certificates' 'gcc' 'libc6-dev'"),
            "in {}",
            made.image_inline
        );
    }
}
