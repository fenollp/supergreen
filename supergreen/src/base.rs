use anyhow::{bail, Result};
use rustc_version::{Channel, Version, VersionMeta};

use crate::{envs::internal, runner::maybe_lock_image};

pub const RUST: &str = "rust-base";

#[derive(Clone, Debug, PartialEq)]
pub struct RustcV {
    pub version: Version,
    pub commit: String,
    pub date: String,
    pub channel: Channel,

    pub base: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BaseImage {
    Image(String),
    RustcV(RustcV),
}

const STABLE_RUST: &str = "docker-image://docker.io/library/rust:1-slim";
const BASE_FOR_RUST: &str = "docker-image://docker.io/library/debian:12-slim";

#[test]
fn test_from_rustc_v() {
    use std::str::FromStr;

    use rustc_version::version_meta_for;

    assert_eq!(
        BaseImage::from_rustcv(
            version_meta_for(
                &r#"
rustc 1.80.0 (051478957 2024-07-21)
binary: rustc
commit-hash: 051478957371ee0084a7c0913941d2a8c4757bb9
commit-date: 2024-07-21
host: x86_64-unknown-linux-gnu
release: 1.80.0
LLVM version: 18.1.7
"#[1..]
            )
            .unwrap()
        ),
        Some(BaseImage::Image("docker-image://docker.io/library/rust:1.80.0-slim".to_owned()))
    );

    assert_eq!(
        BaseImage::from_rustcv(
            version_meta_for(
                &r#"
rustc 1.82.0-nightly (60d146580 2024-08-06)
binary: rustc
commit-hash: 60d146580c10036ce89e019422c6bc2fd9729b65
commit-date: 2024-08-06
host: x86_64-unknown-linux-gnu
release: 1.82.0-nightly
LLVM version: 19.1.0
"#[1..]
            )
            .unwrap()
        ),
        Some(BaseImage::RustcV(RustcV {
            version: Version::from_str("1.82.0-nightly").unwrap(),
            commit: "60d146580c10036ce89e019422c6bc2fd9729b65".to_owned(),
            date: "2024-08-06".to_owned(),
            channel: Channel::Nightly,
            base: "".to_owned(),
        }))
    );
}

impl BaseImage {
    #[inline]
    #[must_use]
    pub fn base(&self) -> String {
        match self {
            Self::Image(img) => img.clone(),
            Self::RustcV(RustcV { base, .. }) => {
                if base.is_empty() {
                    BASE_FOR_RUST.to_owned()
                } else {
                    base.clone()
                }
            }
        }
    }

    pub fn from_rustc_v() -> Result<Self> {
        if let Some(val) = internal::base_image() {
            if !val.starts_with("docker-image://") {
                let var = internal::RUSTCBUILDX_BASE_IMAGE;
                bail!("${var} must start with 'docker-image://' ({val})")
            }
            Ok(Self::Image(val))
        } else {
            Ok(rustc_version::version_meta()
                .ok()
                .and_then(Self::from_rustcv)
                .unwrap_or_else(|| BaseImage::Image(STABLE_RUST.to_owned())))
        }
    }

    #[inline]
    #[must_use]
    fn from_rustcv(
        VersionMeta { semver, commit_hash, commit_date, channel, .. }: VersionMeta,
    ) -> Option<Self> {
        if channel == Channel::Stable {
            return Some(Self::Image(STABLE_RUST.replace(":1-", &format!(":{semver}-"))));
        }
        commit_hash.zip(commit_date).map(|(commit, date)| {
            Self::RustcV(RustcV {
                version: semver.to_owned(),
                commit: commit.to_owned(),
                date: date.to_owned(),
                channel: channel.to_owned(),
                base: "".to_owned(),
            })
        })
    }

    pub async fn maybe_lock_base(self) -> Self {
        self.clone().lock_base_to(maybe_lock_image(self.base()).await)
    }

    pub fn lock_base_to(self, base: String) -> Self {
        match self {
            Self::Image(_) => Self::Image(base),
            Self::RustcV(rustcv @ RustcV { .. }) => Self::RustcV(RustcV { base, ..rustcv }),
        }
    }

    pub fn block(&self) -> String {
        let base = self.base();
        let base = base.trim_start_matches("docker-image://");

        match self {
            Self::Image(_) => format!("FROM {base} AS {RUST}\n"),
            Self::RustcV(RustcV { date, channel, .. }) => {
                // TODO? maybe use commit & version as selector too?

                // FIXME: multiplatformify (using auto ARG.s) (use rustc_version::VersionMeta.host)

                // TODO: use https://github.com/reproducible-containers/repro-sources-list.sh

                let channel = match channel {
                    Channel::Stable => "stable",
                    Channel::Dev => "dev",
                    Channel::Beta => "beta",
                    Channel::Nightly => "nightly",
                };

                format!(
                    r#"
FROM scratch AS rustup
ADD --chmod=0755 --checksum=sha256:6aeece6993e902708983b209d04c0d1dbb14ebb405ddb87def578d41f920f56d \
  https://static.rust-lang.org/rustup/archive/1.27.1/x86_64-unknown-linux-gnu/rustup-init /rustup-init
FROM {base} AS {RUST}
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
 && /rustup-init -y --no-modify-path --profile minimal --default-toolchain {channel}-{date} \
 && chmod -R a+w $RUSTUP_HOME $CARGO_HOME \
 && rustup --version \
 && cargo --version \
 && rustc --version \
 && apt-get remove -y --auto-remove \
 && rm -rf /var/lib/apt/lists/*
"#
                )
            }
        }
    }
}
