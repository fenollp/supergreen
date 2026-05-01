use std::fs::read_to_string;

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use log::info;
use serde::{Deserialize, Serialize};

use crate::{
    base_image::rewrite_cargo_home,
    stage::{AsBlock, AsStage, NamedStage, Stage},
};

pub(crate) const HOME: &str = "git/checkouts";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub(crate) struct Checkouts {
    stage: Stage,
    repo: String,
    commit: String,
    mount: Utf8PathBuf,
}

impl AsBlock for Checkouts {
    fn as_block(&self) -> Option<String> {
        let Self { stage, repo, commit, .. } = self;
        // Add .git suffix, otherwise ADD fetches a webpage, not a repo!
        let repo = if repo.contains("/git.sr.ht/") || repo.contains("@git.sr.ht:") {
            repo
        } else {
            &format!("{repo}.git")
        };
        Some(format!(
            r#"
FROM scratch AS {stage}
ADD --keep-git-dir=false \
  {repo}#{commit} /
"#,
        ))
    }
}

impl AsStage<'_> for Checkouts {
    fn name(&self) -> &Stage {
        &self.stage
    }

    fn mounts(&self) -> Vec<(Option<Utf8PathBuf>, Utf8PathBuf, bool)> {
        vec![(None, self.mount.clone(), false)]
    }
}

/// https://docs.docker.com/reference/dockerfile/#add---keep-git-dir
/// --build-arg BUILDKIT_CONTEXT_KEEP_GIT_DIR=0 https://docs.docker.com/engine/reference/builder/#buildkit-built-in-build-args
pub(crate) async fn as_stage(
    cargo_home: &Utf8Path,
    pkg_manifest_dir: &Utf8Path,
) -> Result<NamedStage> {
    let head = get_remote_origin_url(pkg_manifest_dir).await?;
    info!("opening (RO) git db head file: {head}");
    // e.g.: $CARGO_HOME/git/db/remarkable-tools-9f4e9942cc4e93a3/FETCH_HEAD
    let head = read_to_string(&head).map_err(|e| anyhow!("Failed reading {head}: {e}"))?;
    let head = head.trim();

    let (commit, repo) = commit_and_repo(head)?;
    let repo = repo.trim_end_matches('/');
    let repo = repo.strip_suffix(".git").unwrap_or(repo); // Cleanup here + add it in ADD

    let dir = pkg_manifest_dir.parent().unwrap().file_name().unwrap();
    let stage = Stage::checkout(dir, commit)?;

    let workdir = git_mount(cargo_home, pkg_manifest_dir).expect("we asserted path prefix");

    Ok(NamedStage::Checkouts(Checkouts {
        stage,
        repo: repo.to_owned(),
        commit: commit.to_owned(),
        mount: rewrite_cargo_home(cargo_home, workdir.as_str()).into(),
    }))
}

async fn get_remote_origin_url(pkg_manifest_dir: &Utf8Path) -> Result<Utf8PathBuf> {
    use gix_config::{File, Source};

    // let config_path = pkg_manifest_dir.join(".git/config");
    // e.g.: CARGO_MANIFEST_DIR="$CARGO_HOME/git/checkouts/cross-f0189a1dc141e2d9/88f49ff"
    let (path, _trust) = gix_discover::upwards(pkg_manifest_dir.as_std_path())
        .map_err(|e| anyhow!("Failed getting repository directoy from {pkg_manifest_dir}: {e}"))?;
    let (repository_dir, _worktree_dir) = path.into_repository_and_work_tree_directories();
    let config_path = repository_dir.join("config"); // discovery gives maybe-nonstandard .git folder name

    let config = File::from_path_no_includes(config_path, Source::Local).map_err(|e| {
        anyhow!("Failed getting repository origin url from {pkg_manifest_dir}: {e}")
    })?;

    let url = config
        .string("remote.origin.url")
        .ok_or_else(|| anyhow!("Could not find remote.origin.url from {pkg_manifest_dir}"))?;
    // e.g.: file://$CARGO_HOME/git/db/remarkable-tools-9f4e9942cc4e93a3

    if !url.starts_with("file:///".as_bytes()) {
        bail!("BUG: unexpected repository db path for {pkg_manifest_dir}: {url:?}")
    }
    let db_dir = url["file://".len()..].to_string();
    Ok(Utf8PathBuf::from(db_dir).join("FETCH_HEAD"))
}

fn commit_and_repo(head: &str) -> Result<(&str, &str)> {
    let head = head.lines().last().unwrap();
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

"b06ba3063ff3b3bd0bf419211eb98dcb15dc1b53	not-for-merge	branch 'ctl2' of https://gitlab.inria.fr/coccinelle/coccinelleforrust.git
267449bca2467458f55b7e972602dc9a42eabdba	not-for-merge	branch 'disjunctions' of https://gitlab.inria.fr/coccinelle/coccinelleforrust.git
4dffbb8b59449fcddf1865372e84cf021a9685d7	not-for-merge	branch 'fix_types' of https://gitlab.inria.fr/coccinelle/coccinelleforrust.git
05377dacce6d24bf0b109239d91b13ee3059e84e	not-for-merge	branch 'fixmatch' of https://gitlab.inria.fr/coccinelle/coccinelleforrust.git
cca5eab3fef7c27f9006162c915430d87242c64f	not-for-merge	branch 'get_constants' of https://gitlab.inria.fr/coccinelle/coccinelleforrust.git
f584837349164346bdb25a2b7cbd41db8714fd81	not-for-merge	branch 'get_constants5' of https://gitlab.inria.fr/coccinelle/coccinelleforrust.git
ec0dc54b8947cb702eda8338741222983a58ed5d	not-for-merge	branch 'kangrejos25' of https://gitlab.inria.fr/coccinelle/coccinelleforrust.git
3fb0c2235ebd3aaf9558d0c894a70afe5be57de7	not-for-merge	branch 'macros' of https://gitlab.inria.fr/coccinelle/coccinelleforrust.git
04050b76b29d18d31761e65defada259cc20a28b	not-for-merge	branch 'main' of https://gitlab.inria.fr/coccinelle/coccinelleforrust.git
fe7f52822f070fcecb0414303ab0260f6476e20c	not-for-merge	branch 'match_from_ast' of https://gitlab.inria.fr/coccinelle/coccinelleforrust.git
4178cd4f66b910a16223ed55ede009b016fae1a9	not-for-merge	branch 'new_disj' of https://gitlab.inria.fr/coccinelle/coccinelleforrust.git
2e2312b7858723aa3e605d5365a121e4204ab726	not-for-merge	branch 'rules_add' of https://gitlab.inria.fr/coccinelle/coccinelleforrust.git
bd7179632efe2706027dda332d3db458e49b4b90	not-for-merge	branch 'scripting' of https://gitlab.inria.fr/coccinelle/coccinelleforrust.git
a17c2370938371490f80dcbcf0518eb3dc977c48	not-for-merge	branch 'smpl_parser' of https://gitlab.inria.fr/coccinelle/coccinelleforrust.git
2fed10cf1550e489a02766d16f3d6f45372862d6	not-for-merge	branch 'targetwork' of https://gitlab.inria.fr/coccinelle/coccinelleforrust.git
1b40daa1dddeb263405d8d781e32ceeb3bd98fbe	not-for-merge	branch 'tmp_auto' of https://gitlab.inria.fr/coccinelle/coccinelleforrust.git
a65958e30fcd5be78c8a2f27672ac6aa0112a0b8	not-for-merge	branch 'typeinference' of https://gitlab.inria.fr/coccinelle/coccinelleforrust.git
afdb33ed8111305dd15a26abba89f10ed1c65680	not-for-merge	branch 'wpproblems' of https://gitlab.inria.fr/coccinelle/coccinelleforrust.git
a89c01034a6c17db095c806132ca828bbf1e8830		https://github.com/fenollp/reMarkable-tools.git",

    ];
    let res: Vec<_> = heads.into_iter().map(|head| commit_and_repo(head).unwrap()).collect();
    assert_eq!(res.len(), 3);
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

fn git_mount(cargo_home: &Utf8Path, path: &Utf8Path) -> Option<Utf8PathBuf> {
    if path.starts_with(cargo_home.join(HOME)) {
        let n = cargo_home.components().count() + 2/*HOME's components*/ + 2/*repo's components*/;
        return Some(path.components().take(n).collect());
    }
    None
}

#[test]
fn gitmount() {
    for path in [
        "$CARGO_HOME/git/checkouts/code_reload-a4960c8e3a9a144c/fc16bd2".into(),
        "$CARGO_HOME/git/checkouts/code_reload-a4960c8e3a9a144c/fc16bd2/blip/blop".into(),
    ] {
        assert_eq!(
            Some("$CARGO_HOME/git/checkouts/code_reload-a4960c8e3a9a144c/fc16bd2".into()),
            git_mount("$CARGO_HOME".into(), path)
        );
    }
}
