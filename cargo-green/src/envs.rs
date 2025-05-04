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
