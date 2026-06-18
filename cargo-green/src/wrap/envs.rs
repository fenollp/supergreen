use anyhow::{Result, anyhow};
use camino::Utf8Path;
use log::{debug, trace};

use crate::{
    base_image::{rewrite_cargo_home, rewrite_rustup_home},
    cratesio::rewrite_cratesio_index,
    target_dir::virtual_target_dir_str,
};

pub(crate) fn fmap_env((var, val): (String, String), buildrs: bool) -> Option<(String, String)> {
    let (pass, skip, only_buildrs) = pass_env(&var);
    if pass || (buildrs && only_buildrs) {
        if skip {
            debug!("not forwarding env: {var}={val}");
            return None;
        }
        debug!(
            "env is set: {var}={val} {:?}",
            if var == "CARGO_ENCODED_RUSTFLAGS" {
                rustflags::from_env().collect::<Vec<_>>()
            } else {
                vec![]
            }
        );
        if var == "TERM" {
            debug!("not forwarding {var} ({val})");
            return None;
        }
        if var == "NUM_JOBS" && buildrs {
            // build.rs-only. Not required for recent `cargo`. cc jobserver & CARGO_MAKEFLAGS.
            if val != "1" {
                debug!("overriding {var} ({val})");
            }
            return Some((var, "1".to_owned()));
        }
        return Some((var, val));
    }
    trace!("not passing env: {var}={val}");
    None
}

/// <https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates>
#[must_use]
pub(crate) fn pass_env(var: &str) -> (bool, bool, bool) {
    // Thanks https://github.com/cross-rs/cross/blob/44011c8854cb2eaac83b173cc323220ccdff18ea/src/docker/shared.rs#L969
    let passthrough = [
        "BROWSER",
        "http_proxy",
        "HTTP_TIMEOUT",
        "HTTPS_PROXY",
        "https_proxy",
        "OUT_DIR", // (Only set during compilation.)
        "QEMU_STRACE",
        "RUSTDOCFLAGS",
        "RUSTFLAGS",
        "TERM", // Actually gets skipped later on
    ];
    let skiplist = [
        "CARGO_BUILD_JOBS",                    // TODO? drop
        "CARGO_BUILD_RUSTC",                   // TODO? drop
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER", // TODO? drop
        "CARGO_BUILD_RUSTC_WRAPPER",           // TODO? drop
        "CARGO_BUILD_RUSTDOC",                 // TODO? drop
        "CARGO_BUILD_TARGET_DIR",              // TODO? drop
        "CARGO_HOME",                          // Set in base image
        "CARGO_MAKEFLAGS",                     // TODO: probably drop
        "CARGO_TARGET_DIR",                    // TODO? drop
        "LD_LIBRARY_PATH",                     // TODO: probably drop
        "RUSTC_WORKSPACE_WRAPPER",             // TODO? drop
        "RUSTC_WRAPPER",                       // TODO? drop
        "RUSTUP_HOME",                         // Set in base image
    ];
    let buildrs_only = [
        "DEBUG",
        "HOST",
        "NUM_JOBS",
        "OPT_LEVEL",
        "OUT_DIR",
        "PROFILE",
        "RUSTC", // Will be skipped as it's already set, along with $CARGO
        "RUSTC_LINKER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTC_WRAPPER",
        "RUSTDOC",
        "TARGET",
    ];
    (
        var.starts_with("CARGO_") || passthrough.contains(&var),
        skiplist.contains(&var),
        var.starts_with("DEP_") || buildrs_only.contains(&var),
    )
}

pub(crate) fn safeify(val: &str) -> Result<String> {
    String::from_utf8(shell_quote::Sh::quote_vec(val))
        .map_err(|e| anyhow!("Failed escaping env value {val:?}: {e}"))
        .map(|s| s.replace("\n", "\\\n"))
        .map(|s| if s == "''" { "".to_owned() } else { s })
}

#[test]
fn test_safeify() {
    assert_eq!(safeify("$VAR=val").unwrap(), r#"'$VAR=val'"#.to_owned());
    assert_eq!(
        safeify("the compiler's `proc_macro` API to.").unwrap(),
        r#"the' compiler'\'s' `proc_macro` API to.'"#.to_owned()
    );
    assert_eq!(
        safeify("$VAR=v\na\nl").unwrap(),
        r#"'$VAR=v\
a\
l'"#
        .to_owned()
    );
}

pub(crate) fn rewrite_env(val: &str, cargo_home: &Utf8Path) -> Result<String> {
    let val = safeify(val)?;
    let val = virtual_target_dir_str(&val);
    let val = rewrite_rustup_home(&val);
    let val = rewrite_cratesio_index(&val);
    let val = rewrite_cargo_home(cargo_home, &val);
    // TODO: in rustc's args: replace last WORKDIR with $PWD (--out-dir ..., OUT_DIR=..., maybe others)
    Ok(val)
}

#[test]
fn test_rewrite_env() {
    temp_env::with_var("CARGO_TARGET_DIR", Some("/some/path/"), || {
        let cargo_home: camino::Utf8PathBuf = "/some/other/path".into();

        assert_eq!(
            "https'://github.com/dtolnay/anyhow'",
            rewrite_env("https://github.com/dtolnay/anyhow", &cargo_home).unwrap()
        );

        assert_eq!(
            "$CARGO_HOME/registry/src/index.crates.io'+zstd.1.5.7/zstd/lib'",
            rewrite_env(
                "/some/other/path/registry/src/index.crates.io+zstd.1.5.7/zstd/lib",
                &cargo_home
            )
            .unwrap()
        );

        assert_eq!(
            "/target/release/build/zstd-safe-f387b30b22c9cb23/out",
            rewrite_env("/some/path/release/build/zstd-safe-f387b30b22c9cb23/out", &cargo_home)
                .unwrap()
        );
    });
}
