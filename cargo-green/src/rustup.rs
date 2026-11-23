use std::str::FromStr;

use anyhow::{Error, Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use log::info;
use rustup_toolchain_manifest::{Toolchain, toolchain::Channel};
use serde::Deserialize;

use crate::{all_our_envs::RUSTUP_TOOLCHAIN, sys::fs};

pub(crate) const VERSION: &str = "1.29.0";

pub(crate) static CHECKSUMS: phf::Map<&'static str, &'static str> = phf::phf_map! {
    "aarch64-apple-darwin"      => "aeb4105778ca1bd3c6b0e75768f581c656633cd51368fa61289b6a71696ac7e1",
    "aarch64-unknown-linux-gnu" => "9732d6c5e2a098d3521fca8145d826ae0aaa067ef2385ead08e6feac88fa5792",
    "x86_64-unknown-linux-gnu"  => "4acc9acc76d5079515b46346a485974457b5a79893cfb01112423c89aeb5aa10",
};

/// Path of `$RUSTUP_HOME` eg. `~/.rustup`
pub(crate) fn home() -> Result<Utf8PathBuf> {
    home::rustup_home()
        .map_err(|e| anyhow!("Bad $RUSTUP_HOME or something: {e}"))?
        .try_into()
        .map_err(|e| anyhow!("Corrupted $RUSTUP_HOME path: {e}"))
}

/// Resolve a moving channel to a fixed immutable point release.
///
/// * `$RUSTUP_TOOLCHAIN` is a *name*, and `stable` names a different compiler every six weeks, so
///   hinders hermetic builds (and rebuilds).
/// * NOTE in a target dir, artifacts of one compiler and calls to another produce
///   `error[E0514]: found crate … compiled by an incompatible version of rustc`.
///
/// Rustup keeps the manifest it installed from inside the toolchain, which says both what was
/// released and when.
/// For a channel:
/// * `stable` becomes `<Maj.min.Patch>` eg. `1.98.0`. And with `<Patch>` a release is unique.
/// * `beta` becomes a date. Betas are versioned `1.99.0-beta.3`, and while rustup's grammar
///   allows that (`<channel> = <versioned>[-<prerelease>]`), `rustup_toolchain_manifest` cannot
///   parse it back. `beta-<date>` names the same archive and survives the round trip.
/// * `nightly` becomes a date, having no version of its own to be named by.
pub(crate) fn pinned(toolchain: &str, rustup_home: &Utf8Path) -> Result<Pinned> {
    let (Toolchain { channel, date, .. }, host) = Pinned::parse_into(toolchain)?;

    if date.is_some() || matches!(channel, Channel::Version(_maj, _min, Some(_patch))) {
        return Ok(Pinned { name: toolchain.to_owned(), host }); // Both already name one release
    }

    #[derive(Deserialize)]
    struct Manifest {
        /// Eg. `2026-08-20`: the day this release was cut
        date: String,
        pkg: Pkgs,
    }

    #[derive(Deserialize)]
    struct Pkgs {
        rust: Rust,
    }

    #[derive(Deserialize)]
    struct Rust {
        /// `1.98.0 (88d9e12ae 2026-08-18)`, or `1.99.0-beta.3 (…)` on beta
        version: String,
    }

    let path = rustup_home
        .join("toolchains")
        .join(toolchain)
        .join("lib/rustlib/multirust-channel-manifest.toml");
    let errf = |w, e: Error| anyhow!("Failed {w} manifest of toolchain {toolchain}: {path}: {e}");
    let manifest = fs().read_to_string(&path).map_err(|e| errf("reading", e.into()))?;
    let Manifest { date, pkg: Pkgs { rust: Rust { version } } } =
        toml::from_str(&manifest).map_err(|e| errf("parsing", e.into()))?;

    let pin = match channel {
        Channel::Beta => format!("beta-{date}"),
        Channel::Nightly => format!("nightly-{date}"),
        Channel::Stable | Channel::Version(..) => {
            // Only `<major>.<minor>.<patch>` is wanted: past the first word is a commit and date
            let release = version.split(' ').next().and_then(|v| Toolchain::from_str(v).ok());
            let Some(Channel::Version(major, minor, Some(patch))) = release.map(|t| t.channel)
            else {
                bail!("Unexpected rust version {version:?} in manifest of toolchain {toolchain}")
            };
            format!("{major}.{minor}.{patch}")
        }
    };

    let name = format!("{pin}-{host}");
    info!("pinning {toolchain} to {name}");
    Ok(Pinned { name, host })
}

/// A `$RUSTUP_TOOLCHAIN`: the name of an installed toolchain, and the host it runs on.
///
/// Rustup always sets `$RUSTUP_TOOLCHAIN` to the full name of the toolchain directory it picked,
/// host triple included, even when a shorthand like `stable` was asked for.
/// So there is no need to ask `rustc` which host this is.
#[derive(Debug)]
pub(crate) struct Pinned {
    /// Eg. `1.94.0-x86_64-unknown-linux-gnu`
    pub(crate) name: String,
    /// Eg. `x86_64-unknown-linux-gnu`
    pub(crate) host: String,
}

impl Pinned {
    fn parse_into(toolchain: &str) -> Result<(Toolchain, String)> {
        let parsed = Toolchain::from_str(toolchain)
            .map_err(|e| anyhow!("Failed parsing {RUSTUP_TOOLCHAIN}={toolchain:?}: {e}"))?;
        let Some(host) = parsed.host.as_ref().map(|host| host.target_triple.to_owned()) else {
            bail!("{RUSTUP_TOOLCHAIN}={toolchain:?} names no host triple")
        };
        Ok((parsed, host))
    }

    /// Takes a toolchain name as is, without pinning it to a release
    #[cfg(test)]
    pub(crate) fn parse(toolchain: &str) -> Result<Self> {
        let (_, host) = Self::parse_into(toolchain)?;
        Ok(Self { name: toolchain.to_owned(), host })
    }
}

#[cfg(test)]
mod pinning {
    use super::pinned;
    use crate::sys::{Sys, fake::FakeFs};

    const HOME: &str = "/home/pete/.rustup";

    fn manifest(date: &str, version: &str) -> String {
        format!(
            r#"
manifest-version = "2"
date = "{date}"

[pkg.cargo]
version = "0.99.0 (797e8a9bc 2026-08-05)"

[pkg.rust]
version = "{version} (88d9e12ae 2026-08-18)"

[pkg.rust.target.x86_64-unknown-linux-gnu]
available = true
"#
        )
    }

    fn with(toolchain: &str, manifest: &str) -> String {
        let fs = FakeFs::new();
        fs.pushd(format!("{HOME}/toolchains/{toolchain}/lib/rustlib"));
        fs.file("/multirust-channel-manifest.toml", manifest);
        fs.popd();
        let _guard = Sys::install(Sys { fs, ..Sys::fake() });
        pinned(toolchain, HOME.into()).unwrap().name
    }

    #[test]
    fn stable_becomes_the_version_the_host_runs() {
        let manifest = manifest("2026-08-20", "1.98.0");
        assert_eq!(
            with("stable-x86_64-unknown-linux-gnu", &manifest),
            "1.98.0-x86_64-unknown-linux-gnu"
        );
        // A version without its patch moves with every point release, so it gets pinned too.
        assert_eq!(
            with("1.98-x86_64-unknown-linux-gnu", &manifest),
            "1.98.0-x86_64-unknown-linux-gnu"
        );
    }

    #[test]
    fn a_version_without_its_patch_gets_one() {
        let manifest = manifest("2026-03-05", "1.94.1");
        assert_eq!(
            with("1.94-x86_64-unknown-linux-gnu", &manifest),
            "1.94.1-x86_64-unknown-linux-gnu"
        );
        assert_eq!(with("1.94-aarch64-apple-darwin", &manifest), "1.94.1-aarch64-apple-darwin");
    }

    /// Nightlies are only ever named by date; betas could be named `1.99.0-beta.3`, but
    /// `rustup_toolchain_manifest` does not parse prereleases back, so they are dated too.
    #[test]
    fn prereleases_become_the_date_they_were_cut() {
        assert_eq!(
            with("nightly-x86_64-unknown-linux-gnu", &manifest("2026-03-16", "1.99.0-nightly")),
            "nightly-2026-03-16-x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            with("beta-x86_64-unknown-linux-gnu", &manifest("2026-08-20", "1.99.0-beta.3")),
            "beta-2026-08-20-x86_64-unknown-linux-gnu"
        );
    }

    #[test]
    fn what_is_already_pinned_stays_untouched() {
        let manifest = manifest("2000-01-01", "1.0.0");
        for toolchain in [
            "1.94.0-x86_64-unknown-linux-gnu",
            "nightly-2025-09-14-aarch64-apple-darwin",
            "stable-2026-07-09-x86_64-unknown-linux-gnu",
        ] {
            assert_eq!(with(toolchain, &manifest), toolchain);
        }
    }

    /// Rustup names toolchains, in `$RUSTUP_TOOLCHAIN` and on disk, with their host triple.
    #[test]
    fn a_toolchain_without_a_host_is_an_error() {
        for toolchain in
            ["stable", "1.94", "1.94.0", "nightly-2025-09-14", "nope-x86_64-unknown-linux-gnu"]
        {
            let _guard = Sys::install(Sys::fake());
            assert!(pinned(toolchain, HOME.into()).is_err(), "{toolchain}");
        }
    }

    #[test]
    fn an_unreadable_manifest_is_an_error() {
        let _guard = Sys::install(Sys::fake());
        let err = pinned("stable-x86_64-unknown-linux-gnu", HOME.into()).unwrap_err();
        assert!(err.to_string().contains("Failed reading manifest"), "{err}");
    }

    #[test]
    fn a_manifest_that_names_no_release_is_an_error() {
        for (manifest, expected) in [
            ("manifest-version = \"2\"\n".to_owned(), "Failed parsing manifest"),
            (manifest("2026-08-20", "one.two"), "Unexpected rust version"),
        ] {
            let fs = FakeFs::new();
            fs.pushd(format!("{HOME}/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib"));
            fs.file("/multirust-channel-manifest.toml", manifest);
            fs.popd();
            let _guard = Sys::install(Sys { fs, ..Sys::fake() });
            let err = pinned("stable-x86_64-unknown-linux-gnu", HOME.into()).unwrap_err();
            assert!(err.to_string().contains(expected), "{err}");
        }
    }
}
