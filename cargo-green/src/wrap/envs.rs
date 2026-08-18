use anyhow::{Result, anyhow};
use log::{debug, trace};

pub(crate) fn fmap_env(
    (var, val): (String, String),
    buildrs: bool,
    primary: bool,
) -> Option<(String, String)> {
    let (pass, skip, only_buildrs) = pass_env(&var);
    if pass || (buildrs && only_buildrs) {
        if skip {
            debug!("not forwarding env: {var}={val}");
            return None;
        }
        if var == CARGO_RUSTC_CURRENT_DIR!() && !primary {
            // Only the packages being worked on locate their own sources back through it
            // (`snapbox` & co.), and dependencies must keep compiling in the very same
            // stages as they did before we started setting it: their caches are shared.
            debug!("not forwarding {var} ({val}) to a dependency");
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

/// What crosses into the container decides whether a build is hermetic and cacheable,
/// so pin the policy rather than the (ambient, untestable) environment it runs against.
#[cfg(test)]
mod passing {
    use super::{fmap_env, pass_env};

    /// `(forwarded when building a crate, forwarded when running a build script)`
    ///
    /// Asked of a package being worked on: see [`only_the_worked_on_package_locates_its_sources`]
    /// for what a dependency gets.
    fn verdict(var: &str) -> (bool, bool) {
        let of = |buildrs| fmap_env((var.to_owned(), "v".to_owned()), buildrs, true).is_some();
        (of(false), of(true))
    }

    #[test]
    fn cargo_tells_the_crate_about_itself() {
        for var in
            ["CARGO_PKG_NAME", "CARGO_PKG_VERSION", "CARGO_MANIFEST_DIR", "CARGO_FEATURE_STD"]
        {
            assert_eq!(verdict(var), (true, true), "{var}");
        }
    }

    /// These describe *this host's* cargo invocation, not the crate: forwarding them
    /// would bake machine state into a layer that other machines are meant to reuse.
    #[test]
    fn host_only_cargo_settings_stay_out() {
        for var in [
            "CARGO_HOME",
            "CARGO_TARGET_DIR",
            "CARGO_BUILD_JOBS",
            "CARGO_BUILD_TARGET_DIR",
            "CARGO_MAKEFLAGS",
            "RUSTC_WRAPPER",
            "RUSTUP_HOME",
            "LD_LIBRARY_PATH",
        ] {
            assert_eq!(verdict(var), (false, false), "{var}");
        }
    }

    /// Whole families of cargo settings that can't change rustc's output.
    #[test]
    fn networking_and_terminal_settings_stay_out() {
        for var in [
            "CARGO_NET_OFFLINE",
            "CARGO_HTTP_TIMEOUT",
            "CARGO_TERM_COLOR",
            "CARGO_ALIAS_B",
            "CARGO_REGISTRY_TOKEN",
            "CARGO_REGISTRIES_MY_REGISTRY_TOKEN",
        ] {
            assert_eq!(verdict(var), (false, false), "{var}");
        }
    }

    /// A registry token reaching a layer would be a credential leak into the cache.
    #[test]
    fn registry_credentials_never_leak() {
        let (_, skip, _) = pass_env("CARGO_REGISTRY_TOKEN");
        assert!(skip);
        let (_, skip, _) = pass_env("CARGO_REGISTRIES_CRATES_IO_TOKEN");
        assert!(skip);
    }

    /// Build scripts get a wider set than crate compilation does.
    #[test]
    fn build_script_only_variables() {
        for var in ["TARGET", "HOST", "OPT_LEVEL", "PROFILE", "DEBUG", "DEP_OPENSSL_INCLUDE"] {
            assert_eq!(verdict(var), (false, true), "{var}");
        }
    }

    /// `NUM_JOBS` is pinned so two hosts with different core counts still hit the cache.
    #[test]
    fn num_jobs_is_pinned_to_one() {
        assert_eq!(
            fmap_env(("NUM_JOBS".to_owned(), "32".to_owned()), true, true),
            Some(("NUM_JOBS".to_owned(), "1".to_owned()))
        );
        assert_eq!(fmap_env(("NUM_JOBS".to_owned(), "32".to_owned()), false, true), None);
    }

    /// `$CARGO_RUSTC_CURRENT_DIR` is what test helpers join with `file!()` to read sources
    /// back, so only the packages being worked on have any use for it. Keeping it out of
    /// dependencies leaves their stages exactly as they were, cache hits included.
    #[test]
    fn only_the_worked_on_package_locates_its_sources() {
        let of = |primary| {
            fmap_env(
                (CARGO_RUSTC_CURRENT_DIR!().to_owned(), "/some/path".to_owned()),
                false,
                primary,
            )
        };
        assert_eq!(
            of(true),
            Some((CARGO_RUSTC_CURRENT_DIR!().to_owned(), "/some/path".to_owned()))
        );
        assert_eq!(of(false), None);
    }

    /// There is no terminal in there, and `TERM` would bust the cache per-host.
    #[test]
    fn term_is_dropped_despite_being_listed() {
        let (pass, skip, _) = pass_env("TERM");
        assert!(pass, "listed as passthrough");
        assert!(!skip, "and not skiplisted");
        assert_eq!(verdict("TERM"), (false, false), "yet never forwarded");
    }

    #[test]
    fn rustflags_reach_the_compiler() {
        for var in ["RUSTFLAGS", "RUSTDOCFLAGS", "CARGO_ENCODED_RUSTFLAGS"] {
            assert_eq!(verdict(var), (true, true), "{var}");
        }
    }

    #[test]
    fn unrelated_host_variables_are_ignored() {
        for var in ["HOME", "PATH", "SHELL", "USER", "SSH_AUTH_SOCK", "AWS_SECRET_ACCESS_KEY"] {
            assert_eq!(verdict(var), (false, false), "{var}");
        }
    }
}
