use core::str;
use std::process::Output;

use anyhow::{bail, Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
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
        let Output { stdout, stderr, .. } = cmd.output().await?;
        let commit = str::from_utf8(&stdout).context("parsing git stdout")?.trim();
        if commit.is_empty() {
            bail!("{} failed: {stderr:?}", cmd.show())
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
