use crate::{runner::maybe_lock_image, RUST};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RustcV {
    pub(crate) version: String,
    pub(crate) commit: String,
    pub(crate) date: String,
    pub(crate) channel: String,

    pub(crate) base: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BaseImage {
    Image(String),
    RustcV(RustcV),
}

#[test]
fn test_from_rustc_v() {
    assert_eq!(
        // # rustc -V
        BaseImage::from_rustc_v("rustc 1.79.0 (129f3b996 2024-06-10)\n".to_owned()),
        Some(BaseImage::Image("docker-image://docker.io/library/rust:1.79.0-slim".to_owned()))
    );

    assert_eq!(
        // # rustc +nightly -V
        BaseImage::from_rustc_v("rustc 1.81.0-nightly (d7f6ebace 2024-06-16)\n".to_owned()),
        Some(BaseImage::RustcV(RustcV {
            version: "1.81.0".to_owned(),
            commit: "d7f6ebace".to_owned(),
            date: "2024-06-16".to_owned(),
            channel: "nightly".to_owned(),
            base: "".to_owned(),
        }))
    );
}

impl BaseImage {
    #[inline]
    #[must_use]
    pub(crate) fn base(&self) -> String {
        match self {
            Self::Image(img) => img.clone(),
            Self::RustcV(RustcV { base, .. }) if base.is_empty() => {
                "docker.io/library/debian:12-slim".to_owned()
            }
            Self::RustcV(RustcV { base, .. }) => base.clone(),
        }
    }

    #[inline]
    #[must_use]
    pub(crate) fn from_rustc_v(rustc_v: String) -> Option<Self> {
        let x = rustc_v.trim_start_matches("rustc ").replace(['(', ')', '\n'], "");

        let cut: Vec<&str> = x.splitn(3, ' ').collect();
        let [version, commit, date] = cut[..] else {
            return None;
        };

        for channel in ["nightly", "stable", "beta"] {
            let suffix = format!("-{channel}");
            if version.ends_with(&suffix) {
                return Some(Self::RustcV(RustcV {
                    version: version.trim_end_matches(&suffix).to_owned(),
                    commit: commit.to_owned(),
                    date: date.to_owned(),
                    channel: channel.to_owned(),
                    base: "".to_owned(),
                }));
            }
        }
        Some(BaseImage::Image(format!("docker-image://docker.io/library/rust:{version}-slim")))
    }

    pub(crate) async fn maybe_lock_base(self) -> Self {
        let base = maybe_lock_image(self.base()).await;
        match self {
            Self::Image(_) => Self::Image(base),
            Self::RustcV(rustcv @ RustcV { .. }) => Self::RustcV(RustcV { base, ..rustcv }),
        }
    }

    pub(crate) fn block(&self) -> String {
        let base = self.base();
        let base = base.trim_start_matches("docker-image://");

        if let BaseImage::RustcV(RustcV { version, commit, date, channel, .. }) = self {
            log::trace!("{version} + {commit}");
            let mut block = String::new();

            // FIXME: multiplatformify (using auto ARG.s)

            // TODO: use https://github.com/reproducible-containers/repro-sources-list.sh

            if channel == "nightly" {
                block.push_str(&format!(r#"
FROM {base} AS rustup
RUN \
    set -ux \
 && dpkgArch="$(dpkg --print-architecture)" \
 && case "$dpkgArch" in \
        *amd64) rustArch='x86_64-unknown-linux-gnu' ;; \
        *arm64) rustArch='aarch64-unknown-linux-gnu' ;; \
        *) echo "Unsupported architecture: $dpkgArch" >&2 && exit 1 ;; \
    esac \
 && wget -O /rustup-init https://static.rust-lang.org/rustup/archive/1.27.1/$rustArch/rustup-init \
 && sha256sum /rustup-init \
 && echo '6aeece6993e902708983b209d04c0d1dbb14ebb405ddb87def578d41f920f56d *rustup-init' | sha256sum -c - \
 && chmod +x rustup-init
FROM {base} AS {RUST}
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN \
  --mount=from=rustup,source=/rustup-init,target=/rustup-init \
    set -ux \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
        ca-certificates \
        gcc \
        libc6-dev \
 && /rustup-init -y --no-modify-path --profile minimal --default-toolchain nightly-{date} \
 && chmod -R a+w $RUSTUP_HOME $CARGO_HOME \
 && rustup --version \
 && cargo --version \
 && rustc --version \
 && apt-get remove -y --auto-remove \
 && rm -rf /var/lib/apt/lists/*
"#
                    )[1..],
                );
            }
            block
        } else {
            format!("FROM {base} AS {RUST}\n")
        }
    }
}
