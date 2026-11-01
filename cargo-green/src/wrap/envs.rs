use anyhow::{Result, anyhow};
use log::{debug, trace};

use crate::redact::Redacted;

pub(crate) fn fmap_env<'a>(
    (var, val): (&'a str, &'a str),
    buildrs: bool,
) -> Option<(&'a str, &'a str)> {
    if is_rustflags(var) {
        // Build scripts read the flags their crate's compiled with (e.g. to probe rustc), as
        // we spell them for rustc: see `encode_rustflags`
        if buildrs && var == "CARGO_ENCODED_RUSTFLAGS" && !val.is_empty() {
            debug!(
                "env is set: {:?} {:?}",
                Redacted(var, val),
                rustflags::from_env().collect::<Vec<_>>()
            );
            return Some((var, val));
        }
        debug!(
            "not forwarding env: {:?} (in rustc's arguments already, if at all)",
            Redacted(var, val)
        );
        return None;
    }
    let (pass, skip, only_buildrs) = pass_env(var);
    if pass || (buildrs && only_buildrs) {
        if skip {
            debug!("not forwarding env: {:?}", Redacted(var, val));
            return None;
        }
        debug!("env is set: {:?}", Redacted(var, val));
        if var == "TERM" {
            debug!("not forwarding {:?}", Redacted(var, val));
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
    trace!("not passing env: {:?}", Redacted(var, val));
    None
}

/// The many ways to pass flags to rustc (or rustdoc): `$RUSTFLAGS`, `$CARGO_ENCODED_RUSTFLAGS`,
/// `$CARGO_BUILD_RUSTFLAGS`, `$CARGO_TARGET_<triple>_RUSTFLAGS`, `$CARGO_HOST_RUSTFLAGS`,
/// `$CARGO_PROFILE_<profile>_RUSTFLAGS`, and their `RUSTDOCFLAGS` counterparts.
/// cargo reads them all and passes the flags (that apply) as rustc arguments, which is where we
/// get them from: rustc doesn't read them.
#[must_use]
fn is_rustflags(var: &str) -> bool {
    (var.starts_with("CARGO_") || var.starts_with("RUST"))
        && (var.ends_with("RUSTFLAGS") || var.ends_with("RUSTDOCFLAGS"))
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
        skipprefs.iter().any(|pref| var.starts_with(pref))
            || skiplist.contains(&var)
            || is_target_config(var),
        var.starts_with("DEP_") || buildrs_only.contains(&var),
    )
}

/// `$CARGO_TARGET_<triple>_LINKER` (and `_RUNNER`) is cargo-only config:
/// cargo already hands `-C linker=..` to `rustc`, and `$RUSTC_LINKER` to build scripts.
#[must_use]
fn is_target_config(var: &str) -> bool {
    var.starts_with("CARGO_TARGET_") && (var.ends_with("_LINKER") || var.ends_with("_RUNNER"))
}

#[test]
fn target_config_is_not_passed() {
    for buildrs in [false, true] {
        for var in [
            "CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER",
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER",
        ] {
            assert_eq!(fmap_env((var, "x"), buildrs), None, "{var}");
        }
    }
}

pub(crate) fn safeify(val: &str) -> Result<String> {
    if val.is_empty() {
        return Ok("".to_owned());
    }
    if val.chars().all(|c| c.is_ascii_alphanumeric() || "%+,-./:=@_".contains(c)) {
        return Ok(val.to_owned());
    }
    String::from_utf8(shell_quote::Sh::quote_vec(val))
        .map_err(|e| anyhow!("Failed escaping env value {val:?}: {e}"))
        .map(|s| if s == "''" { "".to_owned() } else { s })
        .map(|s| {
            s.split('\n')
                .map(|line| if line == "EOR" { "'E'OR" } else { line })
                .collect::<Vec<_>>()
                .join("\n")
        })
}

#[test]
fn test_safeify() {
    assert_eq!(safeify("$VAR=val").unwrap(), r#"'$VAR=val'"#.to_owned());

    assert_eq!(safeify("--crate-name=x").unwrap(), "--crate-name=x");
    assert_eq!(
        safeify("-Ldependency=/target/debug/deps").unwrap(),
        "-Ldependency=/target/debug/deps"
    );
    assert_eq!(safeify("--check-cfg=cfg(docsrs,test)").unwrap(), "--check-cfg'=cfg(docsrs,test)'");

    assert_eq!(safeify("").unwrap(), "");

    assert_eq!(
        safeify("the compiler's `proc_macro` API to.").unwrap(),
        r#"the' compiler'\'s' `proc_macro` API to.'"#.to_owned()
    );

    // Newlines as is: RUN scripts are heredocs
    assert_eq!(safeify("$VAR=v\na\nl").unwrap(), "'$VAR=v\na\nl'".to_owned());

    assert_eq!(safeify("EOR").unwrap(), "EOR"); // unescaped: no newline
    assert_eq!(
        safeify("a\nEOR\nb\nEOR\nEOR'").unwrap(),
        // Lines exactly EOR would terminate the RUN heredoc: shell still reads 'E'OR as EOR
        "a'\n'E'OR\nb\n'E'OR\nEOR'\\'".to_owned()
    );
}

#[cfg(test)]
mod you_shall_maybe_pass {
    use super::{fmap_env, is_rustflags, pass_env};

    /// `(forwarded when building a crate, forwarded when /running/ a build script)`
    fn verdict(var: &str) -> (bool, bool) {
        let of = |buildrs| fmap_env((var, "v"), buildrs).is_some();
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

    #[test]
    fn networking_and_terminal_and_any_settings_unaltering_rustc_output_stay_out() {
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

    #[test]
    fn registry_credentials_never_leak() {
        let (_, skip, _) = pass_env("CARGO_REGISTRY_TOKEN");
        assert!(skip);
        let (_, skip, _) = pass_env("CARGO_REGISTRIES_CRATES_IO_TOKEN");
        assert!(skip);
    }

    #[test]
    fn build_scripts_get_a_wider_set_than_crate_compilation_does() {
        for var in ["TARGET", "HOST", "OPT_LEVEL", "PROFILE", "DEBUG", "DEP_OPENSSL_INCLUDE"] {
            assert_eq!(verdict(var), (false, true), "{var}");
        }
    }

    #[test]
    fn num_jobs_is_pinned_so_two_hosts_with_different_core_counts_still_hit_the_cache() {
        assert_eq!(fmap_env(("NUM_JOBS", "32"), true), Some(("NUM_JOBS", "1")));
        assert_eq!(fmap_env(("NUM_JOBS", "32"), false), None);
    }

    #[test]
    fn per_host_term_is_dropped() {
        let (pass, skip, _) = pass_env("TERM");
        assert!(pass, "listed as passthrough");
        assert!(!skip, "and not skiplisted");
        assert_eq!(verdict("TERM"), (false, false), "yet never forwarded");
    }

    #[test]
    fn rustflags_are_rustc_arguments() {
        for var in [
            "RUSTFLAGS",
            "RUSTDOCFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "CARGO_ENCODED_RUSTDOCFLAGS",
            "CARGO_BUILD_RUSTFLAGS",
            "CARGO_BUILD_RUSTDOCFLAGS",
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS",
            "CARGO_HOST_RUSTFLAGS",
            "CARGO_PROFILE_RELEASE_RUSTFLAGS",
        ] {
            for buildrs in [false, true] {
                assert_eq!(fmap_env((var, ""), buildrs), None, "{var}");
                if !(buildrs && var == "CARGO_ENCODED_RUSTFLAGS") {
                    assert_eq!(fmap_env((var, "-Dwarnings"), buildrs), None, "{var}");
                }
            }
        }
        // Build scripts read these
        let flags = "-C\x1ftarget-cpu=native";
        assert_eq!(
            fmap_env(("CARGO_ENCODED_RUSTFLAGS", flags), true),
            Some(("CARGO_ENCODED_RUSTFLAGS", flags))
        );
        // Not just any var ending so
        assert!(!is_rustflags("MY_RUSTFLAGS"));
    }

    #[test]
    fn unrelated_host_variables_are_ignored() {
        for var in ["HOME", "PATH", "SHELL", "USER", "SSH_AUTH_SOCK", "AWS_SECRET_ACCESS_KEY"] {
            assert_eq!(verdict(var), (false, false), "{var}");
        }
    }
}
