use anyhow::{Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use gix_config::{File, Source};
use log::info;
use serde::{Deserialize, Serialize};

use crate::{
    dirs::Paths,
    stage::{AsBlock, AsStage, NamedStage, Stage},
    sys::{fs, git},
};

const HOME: &str = "git/checkouts";

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

/// <https://docs.docker.com/reference/dockerfile/#add---keep-git-dir>
/// `--build-arg BUILDKIT_CONTEXT_KEEP_GIT_DIR=0` <https://docs.docker.com/engine/reference/builder/#buildkit-built-in-build-args>
pub(crate) async fn as_stage(paths: &Paths, pkg_manifest_dir: &Utf8Path) -> Result<NamedStage> {
    let head = git().fetch_head(pkg_manifest_dir)?;
    info!("opening (RO) git db head file: {head}");
    // e.g.: $CARGO_HOME/git/db/remarkable-tools-9f4e9942cc4e93a3/FETCH_HEAD
    let head = fs().read_to_string(&head).map_err(|e| anyhow!("Failed reading {head}: {e}"))?;
    let head = head.trim();

    let (commit, repo) = commit_and_repo(head)?;
    let repo = repo.trim_end_matches('/');
    let repo = repo.strip_suffix(".git").unwrap_or(repo); // Cleanup here + add it in ADD

    let dir = pkg_manifest_dir.parent().unwrap().file_name().unwrap();
    let stage = Stage::checkout(dir, commit)?;

    let workdir = paths.git_mount(pkg_manifest_dir).expect("we asserted path prefix");

    Ok(NamedStage::Checkouts(Checkouts {
        stage,
        repo: repo.to_owned(),
        commit: commit.to_owned(),
        mount: paths.rewrite_cargo_home(workdir.as_str()).into(),
    }))
}

pub(crate) fn real_fetch_head(pkg_manifest_dir: &Utf8Path) -> Result<Utf8PathBuf> {
    // e.g.: "$CARGO_HOME/git/checkouts/cross-f0189a1dc141e2d9/88f49ff"
    let config_path = {
        let (path, _trust) = gix_discover::upwards(pkg_manifest_dir.as_std_path())
            .map_err(|e| anyhow!("Failed getting repository .git/ from {pkg_manifest_dir}: {e}"))?;
        let (repository_dir, _worktree_dir) = path.into_repository_and_work_tree_directories();
        repository_dir.join("config") // discovery gives maybe-nonstandard .git folder name
    };

    let config = File::from_path_no_includes(config_path, Source::Local).map_err(|e| {
        anyhow!("Failed reading repository .git/config from {pkg_manifest_dir}: {e}")
    })?;

    let key = "remote.origin.url";
    let Some(url) = config.string(key) else {
        bail!("Could not find {key} from {pkg_manifest_dir}")
    };
    // e.g.: file://$CARGO_HOME/git/db/remarkable-tools-9f4e9942cc4e93a3

    let url = url.to_string();
    let Some(db_dir) = url.strip_prefix("file://") else {
        bail!("BUG: unexpected repository db path for {pkg_manifest_dir}: {url:?}")
    };
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

impl Paths {
    #[must_use]
    pub(crate) fn checkouts_home(&self) -> Utf8PathBuf {
        self.cargo_home.join(HOME)
    }

    #[must_use]
    pub(crate) fn is_checkout(&self, path: &Utf8Path) -> bool {
        path.starts_with(self.checkouts_home())
    }

    fn git_mount(&self, path: &Utf8Path) -> Option<Utf8PathBuf> {
        if self.is_checkout(path) {
            let n = self.cargo_home.components().count() + 2/*HOME's components*/ + 2/*repo's components*/;
            return Some(path.components().take(n).collect());
        }
        None
    }
}

#[test]
fn gitmount() {
    let paths = Paths { cargo_home: "$CARGO_HOME".into(), ..Default::default() };

    for path in [
        "$CARGO_HOME/git/checkouts/code_reload-a4960c8e3a9a144c/fc16bd2".into(),
        "$CARGO_HOME/git/checkouts/code_reload-a4960c8e3a9a144c/fc16bd2/blip/blop".into(),
    ] {
        assert_eq!(
            Some("$CARGO_HOME/git/checkouts/code_reload-a4960c8e3a9a144c/fc16bd2".into()),
            paths.git_mount(path)
        );
    }
}

/// A crate depended on by git URL becomes an `ADD` of that repo at a pinned commit,
/// so the build fetches the source itself instead of mounting the host's checkout.
#[cfg(test)]
mod as_stage {
    use super::{Paths, as_stage};
    use crate::{
        containerfile::assert_containerfile_eq,
        stage::{AsBlock, AsStage, NamedStage, describe},
        sys::{
            Sys,
            fake::{FakeFs, FakeGit},
        },
    };

    const CHECKOUT: &str = "/home/u/.cargo/git/checkouts/buildxargs-76dd4ee9dadcdcf0/df9b810";
    const DB: &str = "/home/u/.cargo/git/db/buildxargs-76dd4ee9dadcdcf0/FETCH_HEAD";
    const COMMIT: &str = "df9b810011cd416b8e3fc02911f2f496acb8475e";
    const URL: &str = "https://github.com/fenollp/buildxargs.git";

    /// `member` is the crate's path within the repo, `fetch_head` what cargo last fetched.
    fn stage(member: &str, fetch_head: &str) -> NamedStage {
        let manifest_dir = format!("{CHECKOUT}{member}");
        let fs = FakeFs::new();
        fs.file(DB, fetch_head);
        let git = FakeGit::with_head(&manifest_dir, DB);
        let _guard = Sys::install(Sys { fs, git, ..Sys::fake() });

        let paths = Paths { cargo_home: "/home/u/.cargo".into(), ..Default::default() };
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(as_stage(&paths, manifest_dir.as_str().into()))
            .unwrap()
    }

    /// The whole checkout is mounted, from `$CARGO_HOME`, whatever the crate's path in it.
    #[test]
    fn a_crate_at_the_root_of_its_repo() {
        assert_containerfile_eq!(
            describe(&stage("", &format!("{COMMIT}\t\t{URL}"))),
            snapbox::str![[r#"
[Checkouts]
stage = "checkout-buildxargs-76dd4ee9dadcdcf0-df9b810011cd416b8e3fc02911f2f496acb8475e"
repo = "https://github.com/fenollp/buildxargs"
commit = "df9b810011cd416b8e3fc02911f2f496acb8475e"
mount = "$CARGO_HOME/git/checkouts/buildxargs-76dd4ee9dadcdcf0/df9b810"

# as_block
FROM scratch AS checkout-buildxargs-76dd4ee9dadcdcf0-df9b810011cd416b8e3fc02911f2f496acb8475e
ADD --keep-git-dir=false \
  https://github.com/fenollp/buildxargs.git#df9b810011cd416b8e3fc02911f2f496acb8475e /

# mounts
(all) -> $CARGO_HOME/git/checkouts/buildxargs-76dd4ee9dadcdcf0/df9b810

"#]]
        );
    }

    /// The stage is named after the crate's parent dir, so members of one repo@commit
    /// get distinct stages that each `ADD` the same thing.
    /// (corpus: asterinas's `checkout-48c7c37-…`, `checkout-libs-…`, `checkout-linux-bzimage-…`)
    #[test]
    fn workspace_members_each_get_a_stage() {
        let head = format!("{COMMIT}\t\t{URL}");
        let [root, member, nested] = ["", "/member", "/crates/nested"].map(|m| stage(m, &head));

        let names = [&root, &member, &nested].map(|ns| ns.name().to_string());
        assert_eq!(
            names,
            [
                format!("checkout-buildxargs-76dd4ee9dadcdcf0-{COMMIT}"),
                format!("checkout-df9b810-{COMMIT}"),
                format!("checkout-crates-{COMMIT}"),
            ]
        );

        let add = |ns: &NamedStage| ns.as_block().unwrap().lines().skip(2).collect::<String>();
        assert_eq!(add(&root), add(&member));
        assert_eq!(add(&root), add(&nested));
        assert_eq!(root.mounts(), member.mounts());
        assert_eq!(root.mounts(), nested.mounts());
    }

    /// Only FETCH_HEAD's last line is read: its commit need not be the checked out one.
    /// (corpus: coccinelleforrust `ADD`s `86de52a…` yet mounts checkout `50612e2`)
    #[test]
    fn the_commit_is_fetch_head_s_last() {
        let other = "b06ba3063ff3b3bd0bf419211eb98dcb15dc1b53";
        let head = format!(
            "{other}\tnot-for-merge\tbranch 'dev' of {URL}\n{COMMIT}\t\t'{COMMIT}' of {URL}\n"
        );
        let ns = stage("", &head);
        assert!(ns.name().ends_with(COMMIT), "{}", ns.name());
        assert!(!ns.as_block().unwrap().contains(other));
    }

    /// `ADD` needs the `.git` suffix or BuildKit fetches the project's web page, except
    /// on sr.ht which serves repos without it.
    #[test]
    fn repo_urls() {
        for (fetched, added) in [
            (URL, URL),
            ("https://github.com/fenollp/buildxargs", URL),
            ("https://github.com/fenollp/buildxargs/", URL),
            (
                "https://gitlab.inria.fr/coccinelle/coccinelleforrust",
                "https://gitlab.inria.fr/coccinelle/coccinelleforrust.git",
            ),
            (
                "https://fuchsia.googlesource.com/fargo",
                "https://fuchsia.googlesource.com/fargo.git",
            ),
            ("https://git.sr.ht/~someone/somerepo", "https://git.sr.ht/~someone/somerepo"),
            ("git@git.sr.ht:~someone/somerepo", "git@git.sr.ht:~someone/somerepo"),
        ] {
            let block = stage("", &format!("{COMMIT}\t\t{fetched}")).as_block().unwrap();
            assert!(block.contains(&format!("  {added}#{COMMIT} /")), "{fetched}: {block}");
        }
    }
}
