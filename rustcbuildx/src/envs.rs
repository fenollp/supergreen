use std::{
    fs::{File, OpenOptions},
    sync::OnceLock,
};

use anyhow::{anyhow, Result};
use tokio::process::Command;

pub(crate) mod internal {
    use std::env;

    pub const RUSTCBUILDX: &str = "RUSTCBUILDX";
    pub const RUSTCBUILDX_BASE_IMAGE: &str = "RUSTCBUILDX_BASE_IMAGE";
    pub const RUSTCBUILDX_CACHE_IMAGE: &str = "RUSTCBUILDX_CACHE_IMAGE";
    pub const RUSTCBUILDX_INCREMENTAL: &str = "RUSTCBUILDX_INCREMENTAL";
    pub const RUSTCBUILDX_LOG: &str = "RUSTCBUILDX_LOG";
    pub const RUSTCBUILDX_LOG_PATH: &str = "RUSTCBUILDX_LOG_PATH";
    pub const RUSTCBUILDX_LOG_STYLE: &str = "RUSTCBUILDX_LOG_STYLE";
    pub const RUSTCBUILDX_RUNNER: &str = "RUSTCBUILDX_RUNNER";
    pub const RUSTCBUILDX_SYNTAX: &str = "RUSTCBUILDX_SYNTAX";

    pub fn this() -> Option<String> {
        env::var(RUSTCBUILDX).ok()
    }
    pub fn base_image() -> Option<String> {
        env::var(RUSTCBUILDX_BASE_IMAGE).ok()
    }
    pub fn cache_image() -> Option<String> {
        env::var(RUSTCBUILDX_CACHE_IMAGE).ok().and_then(|x| (!x.is_empty()).then_some(x))
    }
    pub fn incremental() -> Option<String> {
        env::var(RUSTCBUILDX_INCREMENTAL).ok()
    }
    pub fn log() -> Option<String> {
        env::var(RUSTCBUILDX_LOG).ok()
    }
    pub fn log_path() -> Option<String> {
        env::var(RUSTCBUILDX_LOG_PATH).ok()
    }
    pub fn log_style() -> Option<String> {
        env::var(RUSTCBUILDX_LOG_STYLE).ok()
    }
    pub fn runner() -> Option<String> {
        env::var(RUSTCBUILDX_RUNNER).ok()
    }
    pub fn syntax() -> Option<String> {
        env::var(RUSTCBUILDX_SYNTAX).ok()
    }
}

// TODO: document envs + usage
// TODO: cli for stats (cache hit/miss/size/age/volume, existing available/selected runners, disk usage/free)

// TODO: cli config / profiles https://github.com/rust-lang/cargo/wiki/Third-party-cargo-subcommands
//   * https://docs.rs/figment/latest/figment/
//   * https://lib.rs/crates/toml_edit
//   * https://github.com/jdrouet/serde-toml-merge
//   * https://crates.io/crates/toml-merge
// https://github.com/cbourjau/cargo-with
// https://github.com/RazrFalcon/cargo-bloat

// If needing additional envs to be passed to rustc or buildrs, set them in the base image.
// RUSTCBUILDX_BASE_IMAGE MUST start with docker-image:// and image MUST be available on DOCKER_HOST e.g.:
// RUSTCBUILDX_BASE_IMAGE=docker-image://rustc_with_libs
// DOCKER_HOST=ssh://oomphy docker buildx build -t rustc_with_libs - <<EOF
// FROM docker.io/library/rust:1.69.0-slim-bookworm@sha256:8bdd28ef184d85c9b4932586af6280732780e806e5f452065420f2e783323ca3
// RUN set -eux && apt update && apt install -y libpq-dev libssl3
// EOF

#[must_use]
pub(crate) fn this() -> bool {
    internal::this().map(|x| x == "1").unwrap_or_default()
}

#[must_use]
pub(crate) fn incremental() -> bool {
    static ONCE: OnceLock<bool> = OnceLock::new();
    *ONCE.get_or_init(|| internal::incremental().map(|x| x == "1").unwrap_or_default())
}

#[must_use]
pub(crate) fn log_path() -> &'static str {
    static ONCE: OnceLock<String> = OnceLock::new();
    ONCE.get_or_init(|| internal::log_path().unwrap_or("/tmp/rstcbldx_FIXME".to_owned()))
}

#[must_use]
pub(crate) fn runner() -> &'static str {
    static ONCE: OnceLock<String> = OnceLock::new();
    ONCE.get_or_init(|| {
        let val = internal::runner().unwrap_or("docker".to_owned());
        // TODO: Result<_> API on bad config
        match val.as_str() {
            "docker" => {}
            "none" => {}
            "podman" => {}
            _ => panic!("{} MUST be either docker, podman or none", internal::RUSTCBUILDX_RUNNER),
        }
        val
    })
}

// A Docker image or any build context, actually.
#[must_use]
pub(crate) async fn base_image() -> &'static str {
    static ONCE: OnceLock<String> = OnceLock::new();
    match ONCE.get() {
        Some(ctx) => ctx,
        None => {
            let ctx = if let Some(val) = internal::base_image() {
                if !val.starts_with("docker-image://") {
                    let var = internal::RUSTCBUILDX_BASE_IMAGE;
                    panic!("{var} must start with 'docker-image://'")
                }
                val
            } else {
                let s = Command::new("rustc").kill_on_drop(true).arg("-V").output().await.ok();
                let s = s.and_then(|child| String::from_utf8(child.stdout).ok());
                // e.g. rustc 1.73.0 (cc66ad468 2023-10-03)

                let v = s
                    .map(|x| x.trim_start_matches("rustc ").to_owned())
                    .and_then(|x| x.split_once(' ').map(|(x, _)| x.to_owned()))
                    .unwrap_or("1".to_owned());

                format!("docker-image://docker.io/library/rust:{v}-slim")
            };

            let ctx = maybe_lock_image(ctx).await;

            let _ = ONCE.set(ctx);
            ONCE.get().expect("just set base_image")
        }
    }
}

// A Docker image path with registry information.
#[must_use]
pub(crate) fn cache_image() -> &'static Option<String> {
    static ONCE: OnceLock<Option<String>> = OnceLock::new();
    ONCE.get_or_init(|| {
        let val = internal::cache_image();

        if let Some(ref val) = val {
            let var = internal::RUSTCBUILDX_CACHE_IMAGE;
            if !val.starts_with("docker-image://") {
                panic!("{var} must start with 'docker-image://'")
            }
            if !val.trim_start_matches("docker-image://").contains('/') {
                panic!("{var} must start with 'docker-image://'")
            }
        }

        val
    })
}

#[must_use]
async fn maybe_lock_image(mut img: String) -> String {
    // Lock image, as podman(4.3.1) does not respect --pull=false (fully, anyway)
    if img.starts_with("docker-image://") && !img.contains("@sha256:") {
        if let Some(line) = Command::new(runner())
            .kill_on_drop(true)
            .arg("inspect")
            .arg("--format={{index .RepoDigests 0}}")
            .arg(img.trim_start_matches("docker-image://"))
            .output()
            .await
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|x| x.lines().next().map(ToOwned::to_owned))
        {
            img.push_str(line.trim_start_matches(|c| c != '@'));
        }
    }
    img
}

#[must_use]
pub(crate) async fn syntax() -> &'static str {
    static ONCE: OnceLock<String> = OnceLock::new();
    match ONCE.get() {
        Some(img) => img,
        None => {
            let img = "docker-image://docker.io/docker/dockerfile:1".to_owned();
            let img = internal::syntax().unwrap_or(img);
            let img = maybe_lock_image(img).await;
            let _ = ONCE.set(img);
            ONCE.get().expect("just set syntax")
        }
    }
}

// TODO: rename proj to https://crates.io/search?q=cargo-surimi

#[must_use]
pub(crate) fn maybe_log() -> Option<fn() -> Result<File>> {
    fn log_file() -> Result<File> {
        let log_path = log_path();
        let errf = |e| anyhow!("Failed opening (WA) log file {log_path}: {e}");
        OpenOptions::new().create(true).append(true).open(log_path).map_err(errf)
    }

    internal::log().map(|x| !x.is_empty()).unwrap_or_default().then_some(log_file)
}

// // See https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-build-scripts
// #[must_use]
// pub(crate) fn called_from_build_script(vars: &BTreeMap<String, String>) -> bool {
//     vars.iter().any(|(k, v)| k.starts_with("CARGO_CFG_") && !v.is_empty())
//         && [
//             "DEBUG",
//             "HOST",
//             "LD_LIBRARY_PATH",
//             "NUM_JOBS",
//             "OPT_LEVEL",
//             "OUT_DIR",
//             "PROFILE",
//             "RUSTC",
//             "RUSTC_LINKER",
//             "RUSTC_WRAPPER",
//             "RUSTDOC",
//             "TARGET",
//         ]
//         .iter()
//         .all(|var| vars.iter().any(|(k, v)| *var == k && !v.is_empty()))
// }

// https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates
#[inline]
#[must_use]
pub(crate) fn pass_env(var: &str) -> (bool, bool) {
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
    // TODO: vvv drop what can be dropped vvv
    let skiplist = [
        "CARGO_BUILD_RUSTC",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTDOC",
        "CARGO_BUILD_TARGET_DIR",
        "CARGO_HOME",
        "CARGO_MAKEFLAGS", // TODO: probably drop
        "CARGO_TARGET_DIR",
        "RUSTC_WRAPPER",
    ];
    (passthrough.contains(&var) || var.starts_with("CARGO_"), skiplist.contains(&var))
}
