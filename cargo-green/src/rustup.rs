use std::str::FromStr;

use anyhow::{Result, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use log::{info, warn};
use rustup_toolchain_manifest::{Toolchain, toolchain::Channel};

use crate::sys::sys;

pub(crate) fn rustup_home() -> Result<Utf8PathBuf> {
    home::rustup_home()
        .map_err(|e| anyhow!("Bad $RUSTUP_HOME or something: {e}"))?
        .try_into()
        .map_err(|e| anyhow!("Corrupted $RUSTUP_HOME path: {e}"))
}

/// Names the release a moving channel stands for on this host, right now.
///
/// `$RUSTUP_TOOLCHAIN` is a *name*, and `stable` names a different compiler every six weeks.
/// Installing that name inside the image resolves it a second time, whenever that layer gets
/// built — so an image can end up a release ahead of (or behind) the host that asked for it.
/// The two only ever meet in a target dir, where artifacts of one compiler and calls to the
/// other produce `error[E0514]: found crate … compiled by an incompatible version of rustc`.
///
/// Rustup writes the channel manifest it installed from into the toolchain itself, dated. That
/// date names the same release to the image as the host is running, and puts the compiler in
/// the recipe, where the results cache key can finally see it.
#[must_use]
pub(crate) fn pinned(toolchain: &str, rustup_home: &Utf8Path) -> String {
    let unchanged = || toolchain.to_owned();

    let Ok(Toolchain { channel, date, host }) = Toolchain::from_str(toolchain) else {
        return unchanged(); // Left to `BaseImage::make_block` to report on
    };
    let channel = match channel {
        // Already names one release, whichever way it was written.
        Channel::Version(..) => return unchanged(),
        _ if date.is_some() => return unchanged(),
        Channel::Stable => "stable",
        Channel::Beta => "beta",
        Channel::Nightly => "nightly",
    };

    let manifest = rustup_home
        .join("toolchains")
        .join(toolchain)
        .join("lib/rustlib/multirust-channel-manifest.toml");
    let Some(date) = sys().fs.read_to_string(&manifest).ok().as_deref().and_then(dated) else {
        warn!("Building against whichever {channel} the image installs: no date in {manifest}");
        return unchanged();
    };

    let host = host.map(|host| format!("-{}", host.target_triple)).unwrap_or_default();
    let pinned = format!("{channel}-{date}{host}");
    info!("pinning {toolchain} to {pinned}");
    pinned
}

/// The date rustup stamped on the manifest a toolchain was installed from.
#[must_use]
fn dated(manifest: &str) -> Option<String> {
    // The whole manifest is megabytes of components; this is its second line.
    manifest
        .lines()
        .take(8)
        .find_map(|line| line.strip_prefix("date = \""))
        .and_then(|date| date.split('"').next())
        .map(ToOwned::to_owned)
}

pub(crate) const VERSION: &str = "1.29.0";

pub(crate) static CHECKSUMS: phf::Map<&'static str, &'static str> = phf::phf_map! {
    "aarch64-apple-darwin"      => "aeb4105778ca1bd3c6b0e75768f581c656633cd51368fa61289b6a71696ac7e1",
    "aarch64-unknown-linux-gnu" => "9732d6c5e2a098d3521fca8145d826ae0aaa067ef2385ead08e6feac88fa5792",
    "x86_64-unknown-linux-gnu"  => "4acc9acc76d5079515b46346a485974457b5a79893cfb01112423c89aeb5aa10",
};

/// The date is the whole point: an image that installs `stable` a week later than the host
/// did installs another compiler, and nothing in the recipe would have said so.
#[cfg(test)]
mod pinning {
    use std::sync::Arc;

    use super::pinned;
    use crate::sys::{Sys, fake::FakeFs, install};

    const HOME: &str = "/home/pete/.rustup";

    fn with_manifest(toolchain: &str, manifest: &str) -> String {
        let fs = Arc::new(FakeFs::default());
        fs.file(
            format!("{HOME}/toolchains/{toolchain}/lib/rustlib/multirust-channel-manifest.toml"),
            manifest,
        );
        let _guard = install(Sys { fs: Arc::clone(&fs) as _, ..Sys::fake() });
        pinned(toolchain, HOME.into())
    }

    #[test]
    fn a_channel_becomes_the_release_the_host_installed() {
        assert_eq!(
            with_manifest(
                "stable-x86_64-unknown-linux-gnu",
                "manifest-version = \"2\"\ndate = \"2026-07-09\"\n[pkg.rust]\n",
            ),
            "stable-2026-07-09-x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            with_manifest("nightly", "manifest-version = \"2\"\ndate = \"2026-03-16\"\n"),
            "nightly-2026-03-16"
        );
    }

    /// Toolchains that already name one release are left exactly as they are.
    #[test]
    fn what_is_already_pinned_stays_untouched() {
        for toolchain in [
            "1.94.0-x86_64-unknown-linux-gnu",
            "1.94.0",
            "nightly-2025-09-14-aarch64-apple-darwin",
            "beta-2026-07-09",
        ] {
            assert_eq!(with_manifest(toolchain, "date = \"2000-01-01\"\n"), toolchain);
        }
    }

    /// Better to build against a moving channel than to fail over a missing file.
    #[test]
    fn an_unreadable_manifest_leaves_the_channel_alone() {
        let _guard = install(Sys::fake());
        assert_eq!(
            pinned("stable-x86_64-unknown-linux-gnu", HOME.into()),
            "stable-x86_64-unknown-linux-gnu"
        );
    }
}
