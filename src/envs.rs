use std::env;

pub(crate) const RUSTCBUILDX: &str = "RUSTCBUILDX";
pub(crate) const RUSTCBUILDX_BASE_IMAGE: &str = "RUSTCBUILDX_BASE_IMAGE";
pub(crate) const RUSTCBUILDX_DOCKER_SYNTAX: &str = "RUSTCBUILDX_DOCKER_SYNTAX";
pub(crate) const RUSTCBUILDX_LOG: &str = "RUSTCBUILDX_LOG";
pub(crate) const RUSTCBUILDX_LOG_IF_CRATE_NAME: &str = "RUSTCBUILDX_LOG_IF_CRATE_NAME";
pub(crate) const RUSTCBUILDX_LOG_PATH: &str = "RUSTCBUILDX_LOG_PATH";
pub(crate) const RUSTCBUILDX_LOG_STYLE: &str = "RUSTCBUILDX_LOG_STYLE";

// TODO: document envs + usage

// RUSTCBUILDX_BASE_IMAGE MUST start with docker-image:// and image MUST be available on DOCKER_HOST e.g.:
// RUSTCBUILDX_BASE_IMAGE=docker-image://rustc_with_libs
// DOCKER_HOST=ssh://oomphy docker buildx build -t rustc_with_libs - <<EOF
// FROM docker.io/library/rust:1.69.0-slim-bookworm@sha256:8bdd28ef184d85c9b4932586af6280732780e806e5f452065420f2e783323ca3
// RUN set -eux && apt update && apt install -y libpq-dev libssl3
// EOF

#[must_use]
pub(crate) fn is_debug() -> bool {
    // TODO: oncelock
    env::var(RUSTCBUILDX_LOG).ok().map(|x| !x.is_empty()).unwrap_or_default()
}

pub(crate) fn log_path() -> String {
    // TODO: oncelock
    env::var(RUSTCBUILDX_LOG_PATH).ok().unwrap_or("/tmp/rstcbldx_FIXME".to_owned())
}

pub(crate) fn base_image() -> String {
    // TODO: oncelock
    // rustc 1.73.0 (cc66ad468 2023-10-03)
    let x="docker-image://docker.io/library/rust:1.73.0-slim@sha256:89e1efffc83a631bced1bf86135f4f671223cc5dc32ebf26ef8b3efd1b97ffff";
    env::var(RUSTCBUILDX_BASE_IMAGE).unwrap_or(x.to_owned())
}

pub(crate) fn docker_syntax() -> String {
    // TODO: oncelock
    let x= "docker.io/docker/dockerfile:1@sha256:ac85f380a63b13dfcefa89046420e1781752bab202122f8f50032edf31be0021";
    env::var(RUSTCBUILDX_DOCKER_SYNTAX).unwrap_or(x.to_owned()) // TODO: see if #syntax= is actually needed
}
