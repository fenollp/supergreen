use std::process::Output;

use anyhow::{bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use cargo_lock::{Lockfile, Package, SourceId};
use pico_args::Arguments;
use serde::Deserialize;
use tokio::process::Command;

use crate::{cargo, extensions::ShowCmd, pwd};

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
    let manifest_path = cargo_locate_project(false).await?;
    let candidate = manifest_path.with_extension("lock");
    if candidate.exists() {
        return Ok(candidate);
    }
    let manifest_path = cargo_locate_project(true).await?;
    Ok(manifest_path.with_extension("lock"))
}

pub(crate) fn find_manifest_path() -> Result<Utf8PathBuf> {
    if let Some(manifest_path) = find_toml_from_env()? {
        return Ok(manifest_path);
    }
    Ok(pwd().join("Cargo.toml"))
}

fn find_toml_from_env() -> Result<Option<Utf8PathBuf>> {
    let mut args = Arguments::from_env();

    let manifest_path: Option<String> = args.opt_value_from_str("--manifest-path")?;
    if let Some(manifest_path) = manifest_path {
        return Ok(Some(manifest_path.into()));
    }

    //FIXME: not true for cinstall cf https://github.com/rust-lang/cargo/issues/9700
    let package: Option<String> = args.opt_value_from_str(["-p", "--package"])?;
    if let Some(package) = package {
        let manifest_path = pwd().join(package).join("Cargo.toml");
        if manifest_path.exists() {
            return Ok(Some(manifest_path));
        }
    }

    Ok(None)
}

// Returns Cargo.toml
// https://doc.rust-lang.org/cargo/commands/cargo-locate-project.html
// https://github.com/rust-lang/cargo/blob/3e96f1a28e47d4fd0f354b3a067d6322a8730cb6/src/bin/cargo/commands/locate_project.rs#L29
async fn cargo_locate_project(at_workspace: bool) -> Result<Utf8PathBuf> {
    let mut cmd = Command::new(cargo());
    cmd.kill_on_drop(true);

    cmd.arg("locate-project");
    if let Some(manifest_path) = find_toml_from_env()? {
        cmd.arg("--manifest-path");
        cmd.arg(manifest_path);
    }
    if at_workspace {
        cmd.arg("--workspace");
    }

    let Output { stderr, stdout, status } = cmd.output().await?;
    if !status.success() || !stderr.is_empty() {
        bail!("{} failed: {:?}", cmd.show(), String::from_utf8_lossy(&stderr))
    }

    #[derive(Debug, Deserialize)]
    struct Located {
        root: Utf8PathBuf,
    }

    let Located { root } = serde_json::from_slice(&stdout)?;
    Ok(root)
}
