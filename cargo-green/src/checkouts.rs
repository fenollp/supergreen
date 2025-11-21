use std::{fs::read_to_string, process::Stdio};

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use log::info;
use tokio::process::Command;

use crate::{ext::CommandExt, stage::Stage};

/// https://docs.docker.com/reference/dockerfile/#add---keep-git-dir
/// --build-arg BUILDKIT_CONTEXT_KEEP_GIT_DIR=0 https://docs.docker.com/engine/reference/builder/#buildkit-built-in-build-args
pub(crate) async fn into_stage(
    krate_manifest_dir: &Utf8Path,
) -> Result<(Stage, Utf8PathBuf, String)> {
    // TODO: replace execve with pure Rust impl, e.g. gitoxide
    let mut cmd = Command::new("git");
    cmd.kill_on_drop(true); // Underlying OS process dies with us
    cmd.stdin(Stdio::null());
    // e.g.: CARGO_MANIFEST_DIR="$CARGO_HOME/git/checkouts/cross-f0189a1dc141e2d9/88f49ff"
    cmd.current_dir(krate_manifest_dir);
    cmd.env_clear(); // Pass all envs explicitly only
    cmd.args(["config", "--get", "remote.origin.url"]);
    let (succeeded, stdout, stderr) = cmd.exec().await?;
    if !succeeded {
        let stderr = String::from_utf8_lossy(&stderr);
        bail!("Failed getting repository origin url: {stderr}")
    }
    let stdout = String::from_utf8_lossy(&stdout);
    let stdout = stdout.trim();
    // e.g.: file:///Users/pierre/.cargo/git/db/remarkable-tools-9f4e9942cc4e93a3
    if !stdout.starts_with("file:///") {
        bail!("BUG: unexpected repository db path: {stdout:?}")
    }
    let db_dir: Utf8PathBuf = stdout.trim_start_matches("file://").into();
    let head = db_dir.join("FETCH_HEAD");

    info!("opening (RO) git db head file: {head}");
    // e.g.: /Users/pierre/.cargo/git/db/remarkable-tools-9f4e9942cc4e93a3/FETCH_HEAD
    let head = read_to_string(&head).map_err(|e| anyhow!("Failed reading {head}: {e}"))?;
    let head = head.trim();

    let (commit, repo) = commit_and_repo(head)?;

    let dir = krate_manifest_dir.parent().unwrap().file_name().unwrap();
    let stage = Stage::checkout(dir, commit)?;

    let block = format!(
        r#"
FROM scratch AS {stage}
ADD --keep-git-dir=false \
  {repo}#{commit} /
"#,
    );

    Ok((stage, krate_manifest_dir.to_owned(), block))
}

fn commit_and_repo(head: &str) -> Result<(&str, &str)> {
    let Some((commit, repo)) = head.split_once("\t\t").map(|(commit, rhs)| {
        let repo = head.split_once("' of ").map(|(_, repo)| repo).unwrap_or(rhs);

        (commit, repo)
    }) else {
        bail!("BUG: unexpected repository head contents: {head:?}")
    };
    Ok((commit, repo))
}

#[test]
fn try_commit_and_repo() {
    let heads = vec![
        "a89c01034a6c17db095c806132ca828bbf1e8830\t\t'a89c01034a6c17db095c806132ca828bbf1e8830' of https://github.com/fenollp/reMarkable-tools.git",
        "a89c01034a6c17db095c806132ca828bbf1e8830\t\thttps://github.com/fenollp/reMarkable-tools.git",
    ];
    let res: Vec<_> = heads.into_iter().map(|head| commit_and_repo(head).unwrap()).collect();
    assert_eq!(res.len(), 2);
    for res in res {
        assert_eq!(
            res,
            (
                "a89c01034a6c17db095c806132ca828bbf1e8830",
                "https://github.com/fenollp/reMarkable-tools.git"
            )
        );
    }
}
