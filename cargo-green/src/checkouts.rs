use core::str;
use std::process::Output;

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use log::info;
use tokio::process::Command;

use crate::{ext::ShowCmd, stage::Stage};

// https://docs.docker.com/reference/dockerfile/#add---keep-git-dir
// --build-arg BUILDKIT_CONTEXT_KEEP_GIT_DIR=0 https://docs.docker.com/engine/reference/builder/#buildkit-built-in-build-args
pub(crate) async fn into_stage(
    krate_manifest_dir: &Utf8Path,
    krate_repository: &str,
) -> Result<(Stage, Utf8PathBuf, String)> {
    let commit = {
        let short = krate_manifest_dir.file_name().unwrap();

        // TODO: replace execve with pure Rust impl, e.g. gitoxide

        let mut cmd = Command::new("git");
        cmd.kill_on_drop(true);
        cmd.args(["rev-parse", "HEAD"]);

        let call = cmd.show_unquoted();
        let envs: Vec<_> = cmd
            .as_std()
            .get_envs()
            .map(|(k, v)| format!("{}={:?}", k.to_string_lossy(), v.unwrap_or_default()))
            .collect();
        let envs = envs.join(" ");

        info!("Calling `{envs} {call}`");

        let Output { stdout, stderr, .. } =
            cmd.output().await.map_err(|e| anyhow!("Failed to spawn `{envs} {call}`: {e}"))?;
        let stdout = String::from_utf8_lossy(&stdout);

        let commit = stdout.trim();
        if commit.is_empty() {
            bail!("`{envs} {call}` failed: {stderr:?}")
        }
        assert!(commit.starts_with(short));
        commit.to_owned()
    };

    let dir = krate_manifest_dir.parent().unwrap().file_name().unwrap();
    let stage = Stage::checkout(dir, &commit)?;

    let repo = if krate_repository.ends_with(".git") {
        krate_repository
    } else {
        &format!("{krate_repository}.git")
    };

    let block = format!(
        r#"
FROM scratch AS {stage}
ADD --keep-git-dir=false \
  {repo}#{commit} /
"#,
    );

    Ok((stage, krate_manifest_dir.to_owned(), block))
}
