use std::env;

pub(crate) const RUSTCBUILDX_DEBUG: &str = "RUSTCBUILDX_DEBUG";
pub(crate) const RUSTCBUILDX_DEBUG_IF_CRATE_NAME: &str = "RUSTCBUILDX_DEBUG_IF_CRATE_NAME";
pub(crate) const RUSTCBUILDX_DOCKER_IMAGE: &str = "RUSTCBUILDX_DOCKER_IMAGE";
pub(crate) const RUSTCBUILDX_DOCKER_SYNTAX: &str = "RUSTCBUILDX_DOCKER_SYNTAX";
pub(crate) const RUSTCBUILDX_SEQUENTIAL_CARGO: &str = "RUSTCBUILDX_SEQUENTIAL_CARGO";
pub(crate) const RUSTCBUILDX_LOG_STYLE: &str = "RUSTCBUILDX_LOG_STYLE";
pub(crate) const RUSTCBUILDX_LOG_PATH: &str = "RUSTCBUILDX_LOG_PATH";
pub(crate) const RUSTCBUILDX_LOG: &str = "RUSTCBUILDX_LOG";

// rustc 1.73.0 (cc66ad468 2023-10-03)
pub(crate) const DOCKER_IMAGE: &str = "docker-image://docker.io/library/rust:1.73.0-slim@sha256:89e1efffc83a631bced1bf86135f4f671223cc5dc32ebf26ef8b3efd1b97ffff";

// TODO: document envs + usage

// RUSTCBUILDX_DOCKER_IMAGE MUST start with docker-image:// and image MUST be available on DOCKER_HOST e.g.:
// RUSTCBUILDX_DOCKER_IMAGE=docker-image://rustc_with_libs
// DOCKER_HOST=ssh://oomphy docker buildx build -t rustc_with_libs - <<EOF
// FROM docker.io/library/rust:1.69.0-slim-bookworm@sha256:8bdd28ef184d85c9b4932586af6280732780e806e5f452065420f2e783323ca3
// RUN set -eux && apt update && apt install -y libpq-dev libssl3
// EOF

pub(crate) const DOCKER_SYNTAX: &str = "docker.io/docker/dockerfile:1@sha256:ac85f380a63b13dfcefa89046420e1781752bab202122f8f50032edf31be0021";

pub(crate) fn is_debug() -> bool {
    // TODO: oncelock
    env::var(RUSTCBUILDX_DEBUG).ok().map(|x| x == "1").unwrap_or_default()
}

pub(crate) fn is_sequential() -> bool {
    // TODO: oncelock
    env::var(RUSTCBUILDX_SEQUENTIAL_CARGO).ok().map(|x| x == "1").unwrap_or_default()
}
