use anyhow::{Result, anyhow};
use log::{debug, trace};

pub(crate) fn fmap_env<'a>(
    (var, val): (&'a str, &'a str),
    buildrs: bool,
) -> Option<(&'a str, &'a str)> {
    let (pass, skip, only_buildrs) = pass_env(var);
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
            return Some((var, "1"));
        }
        return Some((var, val));
    }
    trace!("not passing env: {var}={val}");
    None
}

/// <https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates>
///
/// <https://doc.rust-lang.org/cargo/reference/environment-variables.html#configuration-environment-variables>
///
/// Thanks <https://github.com/cross-rs/cross/blob/44011c8854cb2eaac83b173cc323220ccdff18ea/src/docker/shared.rs#L969>
#[must_use]
pub(crate) fn pass_env(var: &str) -> (bool, bool, bool) {
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
    let skipprefs = [
        // Never affect rustc's inputs
        "CARGO_ALIAS_",
        "CARGO_HTTP_",
        "CARGO_NET_",
        "CARGO_REGISTRIES_",
        "CARGO_REGISTRY_",
        "CARGO_TERM_",
    ];
    let skiplist = [
        "CARGO_BUILD_BUILD_DIR",                  // cargo-only
        "CARGO_BUILD_INCREMENTAL",                // passes '-C incremental=<path>' when true
        "CARGO_BUILD_JOBS",                       // cargo-only
        "CARGO_BUILD_RUSTC",                      // TODO? drop
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",    // TODO? drop
        "CARGO_BUILD_RUSTC_WRAPPER",              // TODO? drop
        "CARGO_BUILD_RUSTDOC",                    // TODO? drop
        "CARGO_BUILD_TARGET_DIR",                 // cargo-only
        "CARGO_BUILD_WARNINGS",                   // cargo-only
        "CARGO_CACHE_AUTO_CLEAN_FREQUENCY",       // cargo-only
        "CARGO_CARGO_NEW_VCS",                    // cargo-only
        "CARGO_FUTURE_INCOMPAT_REPORT_FREQUENCY", // cargo-only
        "CARGO_HOME",                             // Set in base image
        "CARGO_LOG",                              // cargo-only
        "CARGO_MAKEFLAGS",                        // cargo's jobserver subprocesses TODO
        "CARGO_MESSAGE_FORMAT",                   // cargo-only
        "CARGO_TARGET_DIR",                       // cargo-only
        "LD_LIBRARY_PATH",                        // TODO: probably drop
        "RUSTC_WORKSPACE_WRAPPER",                // TODO? drop
        "RUSTC_WRAPPER",                          // TODO? drop
        "RUSTUP_HOME",                            // Set in base image
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
        skipprefs.iter().any(|pref| var.starts_with(pref)) || skiplist.contains(&var),
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
