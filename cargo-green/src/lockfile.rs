use std::env;

use anyhow::{bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use cargo_lock::{Lockfile, Package, SourceId};
use serde::Deserialize;
use tokio::process::Command;

use crate::{extensions::ShowCmd, rustc_wrapper::file_exists_and_is_not_empty};

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
    if file_exists_and_is_not_empty(candidate.as_path())? {
        return Ok(candidate);
    }
    let manifest_path = cargo_locate_project(true).await?;
    Ok(manifest_path.with_extension("lock"))
}

// https://doc.rust-lang.org/cargo/commands/cargo-locate-project.html
// https://github.com/rust-lang/cargo/blob/3e96f1a28e47d4fd0f354b3a067d6322a8730cb6/src/bin/cargo/commands/locate_project.rs#L29
async fn cargo_locate_project(at_workspace: bool) -> Result<Utf8PathBuf> {
    let mut cmd = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.kill_on_drop(true);

    cmd.arg("locate-project");
    if at_workspace {
        cmd.arg("--workspace");
    }

    let output = cmd.output().await?;
    if !output.stderr.is_empty() {
        bail!("{} failed: {:?}", cmd.show(), output.stderr)
    }

    #[derive(Debug, Deserialize)]
    struct Located {
        root: Utf8PathBuf,
    }

    let Located { root } = serde_json::from_slice(&output.stdout)?;
    Ok(root)
}
