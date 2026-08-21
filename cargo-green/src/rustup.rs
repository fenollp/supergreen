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
/// Rustup keeps the manifest it installed from inside the toolchain, which says both what was
/// released and when. Which of the two names a release depends on the channel:
/// * `stable` becomes its version, `1.98.0`: the recipe reads as the compiler it pins, and a
///   patch release is as fine-grained as stable ever gets.
/// * `beta` becomes a date. Betas are versioned `1.99.0-beta.3`, and while rustup's grammar
///   allows that (`<channel> = <versioned>[-<prerelease>]`), `rustup_toolchain_manifest` cannot
///   parse it back. `beta-<date>` names the same archive and survives the round trip.
/// * `nightly` becomes a date, having no version of its own to be named by.
#[must_use]
pub(crate) fn pinned(toolchain: &str, rustup_home: &Utf8Path) -> String {
    let unchanged = || toolchain.to_owned();

    let Ok(Toolchain { channel, date, host }) = Toolchain::from_str(toolchain) else {
        return unchanged(); // Left to `BaseImage::make_block` to report on
    };
    // Both of these already name exactly one release.
    if date.is_some() || matches!(channel, Channel::Version(_, _, Some(_))) {
        return unchanged();
    }

    let manifest = rustup_home
        .join("toolchains")
        .join(toolchain)
        .join("lib/rustlib/multirust-channel-manifest.toml");
    let Ok(manifest) = sys().fs.read_to_string(&manifest) else {
        warn!("Building against whichever {toolchain} the image installs: cannot read {manifest}");
        return unchanged();
    };

    let pin = match channel {
        Channel::Stable | Channel::Version(..) => released(&manifest),
        Channel::Beta => dated(&manifest).map(|date| format!("beta-{date}")),
        Channel::Nightly => dated(&manifest).map(|date| format!("nightly-{date}")),
    };
    let Some(pin) = pin else {
        warn!("Building against whichever {toolchain} the image installs: its manifest names none");
        return unchanged();
    };

    let host = host.map(|host| format!("-{}", host.target_triple)).unwrap_or_default();
    let pinned = format!("{pin}{host}");
    info!("pinning {toolchain} to {pinned}");
    pinned
}

/// The `x.y.z` of the `rust` package a toolchain was installed from.
#[must_use]
fn released(manifest: &str) -> Option<String> {
    let mut lines = manifest.lines().skip_while(|line| *line != "[pkg.rust]");
    let _ = lines.next()?;
    let version = lines
        .take_while(|line| !line.starts_with('['))
        .find_map(|line| line.strip_prefix("version = \""))?;
    // `1.98.0 (88d9e12ae 2026-08-18)`, or `1.99.0-beta.3 (…)` on the beta channel.
    let version = version.split(&[' ', '"'][..]).next()?;
    version.split('.').all(|part| part.parse::<u16>().is_ok()).then(|| version.to_owned())
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

/// A recipe that says `stable` describes no compiler: the image resolves that name again,
/// whenever its layer happens to get built.
#[cfg(test)]
mod pinning {
    use std::sync::Arc;

    use super::pinned;
    use crate::sys::{Sys, fake::FakeFs, install};

    const HOME: &str = "/home/pete/.rustup";

    /// As rustup leaves it inside the toolchain it installed.
    fn manifest(date: &str, version: &str) -> String {
        format!(
            r#"manifest-version = "2"
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

    fn installed(toolchain: &str, manifest: &str) -> String {
        let fs = Arc::new(FakeFs::default());
        fs.file(
            format!("{HOME}/toolchains/{toolchain}/lib/rustlib/multirust-channel-manifest.toml"),
            manifest,
        );
        let _guard = install(Sys { fs: Arc::clone(&fs) as _, ..Sys::fake() });
        pinned(toolchain, HOME.into())
    }

    /// Stable has a version to be named by, and naming it keeps the recipe readable.
    #[test]
    fn stable_becomes_the_version_the_host_runs() {
        let manifest = manifest("2026-08-20", "1.98.0");
        assert_eq!(
            installed("stable-x86_64-unknown-linux-gnu", &manifest),
            "1.98.0-x86_64-unknown-linux-gnu"
        );
        assert_eq!(installed("stable", &manifest), "1.98.0");
        // A version without its patch moves with every point release, so it gets pinned too.
        assert_eq!(installed("1.98", &manifest), "1.98.0");
    }

    /// Nightlies are only ever named by date; betas could be named `1.99.0-beta.3`, but
    /// `rustup_toolchain_manifest` does not parse prereleases back, so they are dated too.
    #[test]
    fn prereleases_become_the_date_they_were_cut() {
        assert_eq!(
            installed(
                "nightly-x86_64-unknown-linux-gnu",
                &manifest("2026-03-16", "1.99.0-nightly")
            ),
            "nightly-2026-03-16-x86_64-unknown-linux-gnu"
        );
        assert_eq!(installed("beta", &manifest("2026-08-20", "1.99.0-beta.3")), "beta-2026-08-20");
    }

    /// Toolchains that already name one release are left exactly as they are.
    #[test]
    fn what_is_already_pinned_stays_untouched() {
        let manifest = manifest("2000-01-01", "1.0.0");
        for toolchain in [
            "1.94.0-x86_64-unknown-linux-gnu",
            "1.94.0",
            "nightly-2025-09-14-aarch64-apple-darwin",
            "stable-2026-07-09",
        ] {
            assert_eq!(installed(toolchain, &manifest), toolchain);
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
        assert_eq!(installed("stable", "manifest-version = \"2\"\n"), "stable");
    }
}
