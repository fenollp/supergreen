use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use cargo_lock::{Lockfile, Package, SourceId};
use pico_args::Arguments;

use crate::pwd;

// TODO: when cargo installing or building without a lockfile
// we can wrap the version picking process to favor cache-hot versions
// Then this'll help: https://github.com/pubgrub-rs/pubgrub

pub(crate) async fn locked_crates(
    manifest_path_lockfile: &Utf8Path,
) -> Result<Vec<(String, String, String)>> {
    let lockfile = Lockfile::load(manifest_path_lockfile)?;

    let packages = lockfile
        .packages
        .into_iter()
        .filter(|pkg| pkg.source.as_ref().is_none_or(SourceId::is_default_registry))
        .filter(|pkg| pkg.checksum.is_some())
        .map(|Package { name, version, checksum, .. }| {
            (name.to_string(), version.to_string(), checksum.unwrap().to_string())
        })
        .collect::<Vec<_>>();

    Ok(packages)
}

pub(crate) async fn find_lockfile() -> Result<Utf8PathBuf> {
    let manifest_path = find_manifest_path().await?;
    let candidate = manifest_path.with_extension("lock");
    if candidate.exists() {
        return Ok(candidate);
    }

    // Fallback on workspace lockfile
    let manifest_path = cargo_metadata(true).await?;
    Ok(manifest_path.with_extension("lock"))
}

/// FIXME: when cargo install-ing, root crate's code isn't local (yet)
/// Meaning accessing its TOML metadata nor its locked deps is possible.
/// cc https://github.com/rust-lang/cargo/issues/9700
pub(crate) async fn find_manifest_path() -> Result<Utf8PathBuf> {
    let mut args = Arguments::from_env();

    let manifest_path: Option<String> = args
        .opt_value_from_str("--manifest-path")
        .map_err(|e| anyhow!("Failed parsing cargo args: {e}"))?;
    if let Some(manifest_path) = manifest_path {
        return Ok(manifest_path.into());
    }

    // This is probably not correct
    let package: Option<String> = args
        .opt_value_from_str(["-p", "--package"])
        .map_err(|e| anyhow!("Failed parsing cargo args: {e}"))?;
    if let Some(package) = package {
        let manifest_path = pwd().join(package).join("Cargo.toml");
        if manifest_path.exists() {
            return Ok(manifest_path);
        }
    }

    cargo_metadata(false).await
}

// TODO: memoize call "for speed"
async fn cargo_metadata(of_workspace: bool) -> Result<Utf8PathBuf> {
    let metadata = cargo_metadata::MetadataCommand::new()
        .no_deps()
        .exec()
        .map_err(|e| anyhow!("Failed running cargo metadata: {e}"))?;

    if of_workspace {
        let Some(root_package) = metadata.packages.first() else {
            bail!("BUG: cargo metadata was not able to find root package")
        };
        Ok(root_package.manifest_path.to_owned())
    } else {
        Ok(metadata.workspace_root.join("Cargo.toml"))
    }
}
