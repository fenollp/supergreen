use std::env;

use anyhow::{bail, Result};
use camino::Utf8PathBuf;
use cargo_lock::{Lockfile, Package, SourceId};
use serde::Deserialize;
use tokio::process::Command;

use crate::rustc_wrapper::file_exists_and_is_not_empty;

pub(crate) async fn locked_crates() -> Result<Vec<(String, String, String)>> {
    let manifest_path_lockfile = find_lockfile().await?;
    let lockfile = Lockfile::load(manifest_path_lockfile)?;

    let packages = lockfile
        .packages
        .into_iter()
        .filter(|pkg| pkg.source.as_ref().is_some_and(SourceId::is_default_registry))
        .filter(|pkg| pkg.checksum.is_some())
        .map(|Package { name, version, checksum, .. }| {
            (name.to_string(), version.to_string(), checksum.unwrap().to_string())
        })
        .collect::<Vec<_>>();
    Ok(packages)
}

async fn find_lockfile() -> Result<Utf8PathBuf> {
    let manifest_path = cargo_locate_project(false).await?;
    let candidate = manifest_path.with_extension("lock");
    if file_exists_and_is_not_empty(candidate.as_path())? {
        return Ok(candidate);
    }
    let manifest_path = cargo_locate_project(true).await?;
    Ok(manifest_path.with_extension("lock"))
}

// https://doc.rust-lang.org/cargo/commands/cargo-locate-project.html
async fn cargo_locate_project(at_workspace: bool) -> Result<Utf8PathBuf> {
    let mut cmd = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.kill_on_drop(true);

    cmd.arg("locate-project");
    if at_workspace {
        cmd.arg("--workspace");
    }

    let output = cmd.output().await?;
    if !output.stderr.is_empty() {
        bail!(">>> {:?}", output.stderr)
    }

    #[derive(Debug, Deserialize)]
    struct Located {
        root: Utf8PathBuf,
    }

    let Located { root } = serde_json::from_slice(&output.stdout)?;
    Ok(root)
}
