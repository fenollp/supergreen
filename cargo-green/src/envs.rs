use std::{
    fs::{File, OpenOptions},
    sync::OnceLock,
};

use anyhow::{anyhow, Result};

use crate::{base::BaseImage, green::Green, runner::maybe_lock_image};

pub(crate) mod internal {
    use std::env;

    pub const RUSTCBUILDX: &str = "RUSTCBUILDX";
    pub const RUSTCBUILDX_BASE_IMAGE: &str = "RUSTCBUILDX_BASE_IMAGE";
    pub const RUSTCBUILDX_BUILDER_IMAGE: &str = "RUSTCBUILDX_BUILDER_IMAGE";
    pub const RUSTCBUILDX_CACHE_IMAGE: &str = "RUSTCBUILDX_CACHE_IMAGE";
    pub const RUSTCBUILDX_INCREMENTAL: &str = "RUSTCBUILDX_INCREMENTAL";
    pub const RUSTCBUILDX_LOG: &str = "RUSTCBUILDX_LOG";
    pub const RUSTCBUILDX_LOG_PATH: &str = "RUSTCBUILDX_LOG_PATH";
    pub const RUSTCBUILDX_LOG_STYLE: &str = "RUSTCBUILDX_LOG_STYLE";
    pub const RUSTCBUILDX_RUNS_ON_NETWORK: &str = "RUSTCBUILDX_RUNS_ON_NETWORK";
    pub const RUSTCBUILDX_SYNTAX: &str = "RUSTCBUILDX_SYNTAX";

    #[must_use]
    pub fn this() -> Option<String> {
        env::var(RUSTCBUILDX).ok()
    }
    #[must_use]
    pub fn base_image() -> Option<String> {
        env::var(RUSTCBUILDX_BASE_IMAGE).ok()
    }
    #[must_use]
    pub fn builder_image() -> Option<String> {
        env::var(RUSTCBUILDX_BUILDER_IMAGE).ok()
    }
    #[must_use]
    pub fn cache_image() -> Option<String> {
        env::var(RUSTCBUILDX_CACHE_IMAGE).ok().and_then(|x| (!x.is_empty()).then_some(x))
    }
    #[must_use]
    pub fn incremental() -> Option<String> {
        env::var(RUSTCBUILDX_INCREMENTAL).ok()
    }
    #[must_use]
    pub fn log() -> Option<String> {
        env::var(RUSTCBUILDX_LOG).ok()
    }
    #[must_use]
    pub fn log_path() -> Option<String> {
        env::var(RUSTCBUILDX_LOG_PATH).ok()
    }
    #[must_use]
    pub fn log_style() -> Option<String> {
        env::var(RUSTCBUILDX_LOG_STYLE).ok()
    }
    #[must_use]
    pub fn runs_on_network() -> Option<String> {
        env::var(RUSTCBUILDX_RUNS_ON_NETWORK).ok()
    }
    #[must_use]
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

// A Docker image or any build context, actually.
#[must_use]
pub(crate) async fn base_image(green: &Green) -> BaseImage {
    static ONCE: OnceLock<BaseImage> = OnceLock::new();
    match ONCE.get() {
        Some(ctx) => ctx.clone(),
        None => {
            let ctx = BaseImage::from_rustc_v().unwrap();

            let ctx = ctx.maybe_lock_base(green).await;

            let _ = ONCE.set(ctx);
            ONCE.get().expect("just set base_image").clone()
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
                panic!("{var} host must contain a registry'")
            }
        }

        // no resolving needed
        // TODO? although we may want to error e.g. when registry is unreachable

        val
    })
}

#[must_use]
pub(crate) fn runs_on_network() -> &'static str {
    static ONCE: OnceLock<String> = OnceLock::new();
    match ONCE.get() {
        Some(network) => network,
        None => {
            let network = internal::runs_on_network().unwrap_or_else(|| "none".to_owned());
            let _ = ONCE.set(network);
            ONCE.get().expect("just set network")
        }
    }
}

pub(crate) const DEFAULT_SYNTAX: &str = "docker-image://docker.io/docker/dockerfile:1";

#[must_use]
pub(crate) async fn syntax(green: &Green) -> &'static str {
    static ONCE: OnceLock<String> = OnceLock::new();
    match ONCE.get() {
        Some(img) => img,
        None => {
            let img = internal::syntax().unwrap_or_else(|| DEFAULT_SYNTAX.to_owned());
            let img = maybe_lock_image(green, &img).await;
            let _ = ONCE.set(img);
            ONCE.get().expect("just set syntax")
        }
    }
}

pub(crate) const DEFAULT_BUILDER_IMAGE: &str =
    "docker-image://docker.io/moby/buildkit:buildx-stable-1";

#[must_use]
pub(crate) async fn builder_image(green: &Green) -> &'static str {
    static ONCE: OnceLock<String> = OnceLock::new();
    match ONCE.get() {
        Some(img) => img,
        None => {
            let img = internal::builder_image().unwrap_or_else(|| DEFAULT_BUILDER_IMAGE.to_owned());
            let img = maybe_lock_image(green, &img).await;
            let _ = ONCE.set(img);
            ONCE.get().expect("just set builder_image")
        }
    }
}

#[must_use]
pub(crate) fn maybe_log() -> Option<fn() -> Result<File>> {
    fn log_file() -> Result<File> {
        let log_path = log_path();
        let errf = |e| anyhow!("Failed opening (WA) log file {log_path}: {e}");
        OpenOptions::new().create(true).append(true).open(log_path).map_err(errf)
    }

    internal::log().map(|x| !x.is_empty()).unwrap_or_default().then_some(log_file)
}

// https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates
#[must_use]
pub(crate) fn pass_env(var: &str) -> (bool, bool, bool) {
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
        "CARGO_BUILD_JOBS",
        "CARGO_BUILD_RUSTC",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTDOC",
        "CARGO_BUILD_TARGET_DIR",
        "CARGO_HOME",      // TODO? drop
        "CARGO_MAKEFLAGS", // TODO: probably drop
        "CARGO_TARGET_DIR",
        "LD_LIBRARY_PATH", // TODO: probably drop
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
    ];
    let buildrs_only = [
        "DEBUG",
        "HOST",
        "NUM_JOBS",
        "OPT_LEVEL",
        "OUT_DIR",
        "PROFILE",
        "RUSTC",
        "RUSTC_LINKER",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTDOC",
        "TARGET",
    ];
    (
        var.starts_with("CARGO_") || passthrough.contains(&var),
        skiplist.contains(&var),
        var.starts_with("DEP_") || buildrs_only.contains(&var),
    )
}
