use crate::{runner::maybe_lock_image, RUST};

// λ rustc -V
// rustc 1.79.0 (129f3b996 2024-06-10)
// λ rustc +nightly -V
// rustc 1.81.0-nightly (8337ba918 2024-06-12)
#[derive(Clone, Debug)]
pub(crate) struct RustcV {
    pub(crate) version: String,
    pub(crate) commit: String,
    pub(crate) date: String,
    pub(crate) channel: String,

    base: String,
}

#[derive(Clone, Debug)]
pub(crate) enum BaseImage {
    Image(String),
    RustcV(RustcV),
}

impl BaseImage {
    #[inline]
    #[must_use]
    pub(crate) fn base(&self) -> String {
        match self {
            Self::Image(img) => img.trim_start_matches("docker-image://").to_owned(),
            Self::RustcV(RustcV { base, .. }) if base.is_empty() => {
                "docker.io/library/debian:12-slim".to_owned()
            }
            Self::RustcV(RustcV { base, .. }) => base.clone(),
        }
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
 &&     dpkgArch="$(dpkg --print-architecture)"; \
    case "$dpkgArch" in \
        *amd64) rustArch='x86_64-unknown-linux-gnu' ;; \
        *arm64) rustArch='aarch64-unknown-linux-gnu' ;; \
        *) echo >&2 "unsupported architecture: $dpkgArch"; exit 1 ;; \
    esac; \
 && wget -O /rustup-init https://static.rust-lang.org/rustup/archive/1.27.1/$rustArch/rustup-init \
 && sha256sum /rustup-init \
 && echo "6aeece6993e902708983b209d04c0d1dbb14ebb405ddb87def578d41f920f56d *rustup-init" | sha256sum -c - \
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
