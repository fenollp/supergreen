use std::iter::once;

use anyhow::{Result, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};

use crate::{
    dirs::virtual_target_dir,
    md::MdId,
    stage::{AsBlock, AsStage, NamedStage, Stage},
    sys::fs,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub(crate) struct Relative {
    stage: Stage,
    pwd: Utf8PathBuf,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    keep: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    lose: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dockerignore: Option<Utf8PathBuf>,
}

impl AsBlock for Relative {}

impl AsStage<'_> for Relative {
    fn name(&self) -> &Stage {
        &self.stage
    }

    fn mounts(&self) -> Vec<(Option<Utf8PathBuf>, Utf8PathBuf, bool)> {
        let Self { keep, pwd, .. } = self;
        keep.iter()
            .map(|fname| (Some(format!("/{fname}").into()), format!("{pwd}/{fname}").into(), true))
            .collect()
    }

    fn context(&mut self) -> Option<(Stage, Utf8PathBuf)> {
        let Self { stage, lose, pwd, .. } = self;
        if !lose.is_empty() {
            let dockerignore = pwd.join(".dockerignore");
            let already_has_one = fs().exists(&dockerignore);
            //FIXME: if exists: save + extend (then restore??) .dockerignore
            //TODO? add .gitignore in there?
            //TODO? exclude everything, only include `git ls-files`?

            let mut lose: Vec<String> = lose
                .iter()
                .chain(once(&".dockerignore".to_owned()))
                .map(|fname| format!("/{fname}\n"))
                .collect();
            lose.sort();
            lose.dedup();
            let lose: String = lose.into_iter().collect();
            if let Err(e) = fs().write(&dockerignore, &lose) {
                warn!("Failed writing {dockerignore}: {e}");
            }

            if !already_has_one {
                self.dockerignore = Some(dockerignore);
            }
        }
        Some((stage.to_owned(), pwd.to_owned()))
    }
}

impl Drop for Relative {
    fn drop(&mut self) {
        if let Some(ref dockerignore) = self.dockerignore {
            let _ = fs().remove_file(dockerignore);
        }
    }
}

/// NOTE: build contexts have to be directories, can't be files.
/// ```
/// failed to get build context path {$HOME/wefwefwef/supergreen.git/Cargo.lock <nil>}: not a directory
/// ```
pub(crate) async fn as_stage(mdid: MdId, pwd: &Utf8Path) -> Result<NamedStage> {
    info!("mounting {}files under {pwd}", if fs().is_dir(&pwd.join(".git")) { "git " } else { "" });

    let (keep, lose) = {
        let mut entries =
            fs().read_dir(pwd).map_err(|e| anyhow!("Failed reading dir {pwd:?}: {e}"))?;
        entries.sort(); // deterministic iteration
        entries.iter().filter_map(|p| p.file_name()).map(ToOwned::to_owned).partition(|fname| {
            if fname == ".dockerignore" {
                debug!("excluding {fname}");
                return false;
            }
            if fname == virtual_target_dir().trim_start_matches('/') {
                debug!("excluding {fname} or it will clash with internal target dir");
                return false;
            }
            if fname == ".git" && fs().is_dir(&pwd.join(fname)) {
                debug!("excluding {fname} dir");
                return false; // Skip copying .git dir
            }
            if fs().exists(&pwd.join(fname).join("CACHEDIR.TAG")) {
                debug!("excluding {fname} dir");
                return false; // Test for existence of ./target/CACHEDIR.TAG See https://bford.info/cachedir/
            }
            debug!("keeping {fname}");
            true
        })
    };

    Ok(NamedStage::Relative(Relative {
        stage: Stage::local(mdid)?,
        pwd: pwd.to_owned(),
        keep,
        lose,
        dockerignore: None,
    }))
}

/// Local code is a build context: only the crate dir's top-level entries that belong to
/// the source are mounted, the rest is `.dockerignore`d while the build runs.
#[cfg(test)]
mod as_stage {
    use std::sync::Arc;

    use super::as_stage;
    use crate::{
        stage::{AsStage, NamedStage},
        sys::{Sys, fake::FakeFs},
    };

    const PWD: &str = "/home/u/mycrate";
    const CRATE: [&str; 4] = ["/Cargo.toml", "/src/main.rs", "/.git/HEAD", "/target/CACHEDIR.TAG"];

    /// A crate dir holding `files` (relative to it).
    fn crate_dir(files: &[&str]) -> Arc<FakeFs> {
        let fs = FakeFs::new();
        fs.pushd(PWD);
        files.iter().for_each(|file| fs.file(file, ""));
        fs.popd();
        fs
    }

    fn stage() -> NamedStage {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(as_stage(0x5555555555555555_u64.into(), PWD.into()))
            .unwrap()
    }

    #[test]
    fn a_crate_dir() {
        let _guard = Sys::install(Sys { fs: crate_dir(&CRATE), ..Sys::fake() });
        assert_snapshots_eq!(
            stage().describe(),
            snapbox::str![[r#"
[Relative]
stage = "cwd-5555555555555555"
pwd = "/home/u/mycrate"
keep = [
    "Cargo.toml",
    "src",
]
lose = [
    ".git",
    "target",
]

# as_block
(none)

# mounts
/Cargo.toml -> /home/u/mycrate/Cargo.toml
/src -> /home/u/mycrate/src

"#]]
        );
    }

    /// Left out by kind, not by name (`target` aside, which would shadow ours).
    #[test]
    fn what_is_left_out() {
        let fs = crate_dir(&[
            "/Cargo.toml",
            "/.dockerignore",       // Ours to write
            "/build/CACHEDIR.TAG",  // A --target-dir by another name
            "/target/debug/x",      // Untagged
            "/.jj/repo/store/type", // Not special
        ]);
        let _guard = Sys::install(Sys { fs, ..Sys::fake() });
        assert_snapshots_eq!(
            stage().describe(),
            snapbox::str![[r#"
[Relative]
stage = "cwd-5555555555555555"
pwd = "/home/u/mycrate"
keep = [
    ".jj",
    "Cargo.toml",
]
lose = [
    ".dockerignore",
    "build",
    "target",
]

# as_block
(none)

# mounts
/.jj -> /home/u/mycrate/.jj
/Cargo.toml -> /home/u/mycrate/Cargo.toml

"#]]
        );
    }

    /// In a worktree or submodule, `.git` is a file pointing elsewhere: it is kept.
    #[test]
    fn a_git_file_is_kept() {
        let _guard = Sys::install(Sys { fs: crate_dir(&["/Cargo.toml", "/.git"]), ..Sys::fake() });
        assert_snapshots_eq!(
            stage().describe(),
            snapbox::str![[r#"
[Relative]
stage = "cwd-5555555555555555"
pwd = "/home/u/mycrate"
keep = [
    ".git",
    "Cargo.toml",
]

# as_block
(none)

# mounts
/.git -> /home/u/mycrate/.git
/Cargo.toml -> /home/u/mycrate/Cargo.toml

"#]]
        );
    }

    /// The `.dockerignore` lists what was left out and lasts as long as the stage.
    #[test]
    fn the_dockerignore_lives_as_long_as_the_stage() {
        let fs = crate_dir(&CRATE);
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });

        let mut ns = stage();
        let _ = ns.context();
        assert_snapshots_eq!(
            fs.read(format!("{PWD}/.dockerignore")).unwrap(),
            snapbox::str![[r#"
/.dockerignore
/.git
/target

"#]]
        );
        assert_snapshots_eq!(
            toml::to_string_pretty(&ns).unwrap(),
            snapbox::str![[r#"
[Relative]
stage = "cwd-5555555555555555"
pwd = "/home/u/mycrate"
keep = [
    "Cargo.toml",
    "src",
]
lose = [
    ".git",
    "target",
]
dockerignore = "/home/u/mycrate/.dockerignore"

"#]]
        );

        drop(ns);
        assert_eq!(fs.read(format!("{PWD}/.dockerignore")), None);
    }

    /// FIXME: a user's `.dockerignore` is overwritten, and left that way.
    #[test]
    fn a_user_dockerignore_is_clobbered() {
        let fs = crate_dir(&CRATE);
        fs.file(format!("{PWD}/.dockerignore"), "/secrets\n");
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });

        let mut ns = stage();
        let _ = ns.context();
        drop(ns);
        assert_snapshots_eq!(
            fs.read(format!("{PWD}/.dockerignore")).unwrap(),
            snapbox::str![[r#"
/.dockerignore
/.git
/target

"#]]
        );
    }
}
