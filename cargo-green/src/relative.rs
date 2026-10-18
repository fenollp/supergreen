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

    /// What got appended to a user's `.dockerignore`, to remove once done.
    #[serde(skip)]
    appended: Option<String>,
}

const APPEND_HEAD: &str = "\n# Added by cargo-green, while it builds\n";

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

            if !fs().exists(&dockerignore) {
                if let Err(e) = fs().write(&dockerignore, &lose) {
                    warn!("Failed writing {dockerignore}: {e}");
                }
                self.dockerignore = Some(dockerignore);
            } else {
                // A user's: extended for the time being
                // TODO: special case on symlink
                let appended = format!("{APPEND_HEAD}{lose}");
                match fs().read_to_string(&dockerignore) {
                    Ok(theirs) if theirs.ends_with(&appended) => {} // Some sibling call's doing
                    Ok(_) => match fs().append(&dockerignore, &appended) {
                        Ok(()) => self.appended = Some(appended),
                        Err(e) => warn!("Failed extending {dockerignore}: {e}"),
                    },
                    Err(e) => warn!("Failed reading {dockerignore}: {e}"),
                }
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
        if let Some(ref appended) = self.appended {
            let dockerignore = self.pwd.join(".dockerignore");
            if let Ok(txt) = fs().read_to_string(&dockerignore)
                && let Some(theirs) = txt.strip_suffix(appended)
            {
                let _ = fs().write(&dockerignore, theirs);
            }
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
        appended: None,
    }))
}

#[cfg(test)]
mod as_stage {
    use std::sync::Arc;

    use super::as_stage;
    use crate::{
        stage::{AsStage, NamedStage},
        sys::{Sys, fake::FakeFs},
    };

    macro_rules! CWD {
        ($path:literal) => {
            concat!("/home/u/mycrate", $path)
        };
    }

    const CRATE: &[&str] = &["/Cargo.toml", "/src/main.rs", "/.git/HEAD", "/target/CACHEDIR.TAG"];

    fn with(files: &[&str], f: impl AsyncFnOnce(NamedStage, Arc<FakeFs>) -> ()) {
        let fs = FakeFs::new();
        fs.pushd(CWD!(""));
        for fname in files {
            fs.file(fname, "");
        }
        fs.popd();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
            let stage = as_stage(0x5555555555555555_u64.into(), CWD!("").into()).await.unwrap();
            f(stage, fs).await;
        });
    }

    #[test]
    fn some_crate_dir() {
        with(CRATE, async |stage, _fs| {
            snapbox::assert_data_eq!(
                stage.describe(),
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
...
# mounts
/Cargo.toml -> /home/u/mycrate/Cargo.toml
/src -> /home/u/mycrate/src

"#]]
            );
        });
    }

    #[test]
    fn what_is_left_out() {
        let files = [
            "/Cargo.toml",
            "/.dockerignore",      // Ours to write
            "/build/CACHEDIR.TAG", // A --target-dir by another name
            "/target/debug/x",
            "/.jj/repo/store/type", // Not special
            "/.git",                // Kept
        ];
        with(&files, async |stage, _fs| {
            snapbox::assert_data_eq!(
                stage.describe(),
                snapbox::str![[r#"
[Relative]
stage = "cwd-5555555555555555"
pwd = "/home/u/mycrate"
keep = [
    ".git",
    ".jj",
    "Cargo.toml",
]
lose = [
    ".dockerignore",
    "build",
    "target",
]
...
# mounts
/.git -> /home/u/mycrate/.git
/.jj -> /home/u/mycrate/.jj
/Cargo.toml -> /home/u/mycrate/Cargo.toml

"#]]
            );
        });
    }

    #[test]
    fn the_dockerignore_lives_as_long_as_the_stage() {
        with(CRATE, async |mut stage, fs| {
            let _ = stage.context(); // Touches .dockerignore
            snapbox::assert_data_eq!(
                toml::to_string_pretty(&stage).unwrap(),
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
            snapbox::assert_data_eq!(
                fs.read(CWD!("/.dockerignore")).unwrap(),
                snapbox::str![[r#"
/.dockerignore
/.git
/target

"#]]
            );
            drop(stage);
            assert_eq!(fs.read(CWD!("/.dockerignore")), None);
        });
    }

    /// A user's `.dockerignore` is extended while the stage lasts, then left as it was.
    #[test]
    fn a_user_dockerignore_is_kept() {
        with(CRATE, async |mut stage, fs| {
            fs.file(CWD!("/.dockerignore"), "/secrets\n");
            let _ = stage.context(); // Touches .dockerignore
            snapbox::assert_data_eq!(
                fs.read(CWD!("/.dockerignore")).unwrap(),
                snapbox::str![[r#"
/secrets

# Added by cargo-green, while it builds
/.dockerignore
/.git
/target

"#]]
            );
            // Another call for the same crate, at the same time
            let mut sibling =
                as_stage(0x5555555555555555_u64.into(), CWD!("").into()).await.unwrap();
            let _ = sibling.context();
            drop(sibling);
            drop(stage);
            assert_eq!(fs.read(CWD!("/.dockerignore")).unwrap(), "/secrets\n");
        });
    }
}
