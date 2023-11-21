use std::{
    env,
    fs::{File, OpenOptions},
};

use anyhow::{Context, Result};

pub(crate) const RUSTCBUILDX: &str = "RUSTCBUILDX";
pub(crate) const RUSTCBUILDX_BASE_IMAGE: &str = "RUSTCBUILDX_BASE_IMAGE";
pub(crate) const RUSTCBUILDX_DOCKER_SYNTAX: &str = "RUSTCBUILDX_DOCKER_SYNTAX";
pub(crate) const RUSTCBUILDX_LOG: &str = "RUSTCBUILDX_LOG";
pub(crate) const RUSTCBUILDX_LOG_PATH: &str = "RUSTCBUILDX_LOG_PATH";
pub(crate) const RUSTCBUILDX_LOG_STYLE: &str = "RUSTCBUILDX_LOG_STYLE";

// TODO: document envs + usage

// If needing additional envs to be passed to rustc or buildrs, set them in the base image.
// RUSTCBUILDX_BASE_IMAGE MUST start with docker-image:// and image MUST be available on DOCKER_HOST e.g.:
// RUSTCBUILDX_BASE_IMAGE=docker-image://rustc_with_libs
// DOCKER_HOST=ssh://oomphy docker buildx build -t rustc_with_libs - <<EOF
// FROM docker.io/library/rust:1.69.0-slim-bookworm@sha256:8bdd28ef184d85c9b4932586af6280732780e806e5f452065420f2e783323ca3
// RUN set -eux && apt update && apt install -y libpq-dev libssl3
// EOF

pub(crate) fn log_path() -> String {
    env::var(RUSTCBUILDX_LOG_PATH).ok().unwrap_or("/tmp/rstcbldx_FIXME".to_owned())
}

pub(crate) fn base_image() -> String {
    // rustc 1.73.0 (cc66ad468 2023-10-03)
    let x="docker-image://docker.io/library/rust:1.73.0-slim@sha256:89e1efffc83a631bced1bf86135f4f671223cc5dc32ebf26ef8b3efd1b97ffff";
    env::var(RUSTCBUILDX_BASE_IMAGE).unwrap_or(x.to_owned())
}

pub(crate) fn docker_syntax() -> String {
    let x= "docker.io/docker/dockerfile:1@sha256:ac85f380a63b13dfcefa89046420e1781752bab202122f8f50032edf31be0021";
    env::var(RUSTCBUILDX_DOCKER_SYNTAX).unwrap_or(x.to_owned()) // TODO: see if #syntax= is actually needed
}

pub(crate) fn maybe_log() -> Option<fn() -> Result<File>> {
    fn log_file() -> Result<File> {
        let log_path = log_path();
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("Failed opening (WA) log file {log_path}"))
    }

    env::var(RUSTCBUILDX_LOG).ok().map(|x| !x.is_empty()).unwrap_or_default().then_some(log_file)
}

// See https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-build-scripts
#[must_use]
pub(crate) fn called_from_build_script() -> bool {
    env::vars().any(|(k, v)| k.starts_with("CARGO_CFG_") && !v.is_empty())
        && ["HOST", "NUM_JOBS", "OUT_DIR", "PROFILE", "TARGET"]
            .iter()
            .all(|var| env::vars().any(|(k, v)| *var == k && !v.is_empty()))
}

// https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates
#[inline]
#[must_use]
pub(crate) fn pass_env(var: &str) -> bool {
    // Thanks https://github.com/cross-rs/cross/blob/44011c8854cb2eaac83b173cc323220ccdff18ea/src/docker/shared.rs#L969
    let passthrough = [
        "http_proxy",
        "TERM",
        "RUSTDOCFLAGS",
        "RUSTFLAGS",
        "BROWSER",
        "HTTPS_PROXY",
        "HTTP_TIMEOUT",
        "https_proxy",
        "QEMU_STRACE",
        // Not here but set in RUN script: CARGO, PATH, ...
        "OUT_DIR", // (Only set during compilation.)
    ];
    let skiplist = [
        "CARGO_BUILD_RUSTC",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTDOC",
        "CARGO_BUILD_TARGET_DIR",
        "CARGO_HOME",
        "CARGO_TARGET_DIR",
        "RUSTC_WRAPPER",
    ];
    (passthrough.contains(&var) || var.starts_with("CARGO_")) && !skiplist.contains(&var)
}
