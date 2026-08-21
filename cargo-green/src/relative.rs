use std::iter::once;

use anyhow::{Result, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};

use crate::{
    dirs::is_named_same_as_virtual_target_dir,
    md::MdId,
    stage::{AsBlock, AsStage, NamedStage, Stage},
    sys::sys,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub(crate) struct Relative {
    stage: Stage,
    pwd: Utf8PathBuf,
    keep: Vec<String>,
    lose: Vec<String>,
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
            let already_has_one = sys().fs.exists(&dockerignore);
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
            if let Err(e) = sys().fs.write(&dockerignore, &lose) {
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
            let _ = sys().fs.remove_file(dockerignore);
        }
    }
}

/// NOTE: build contexts have to be directories, can't be files.
/// ```
/// failed to get build context path {$HOME/wefwefwef/supergreen.git/Cargo.lock <nil>}: not a directory
/// ```
pub(crate) async fn as_stage(mdid: MdId, pwd: &Utf8Path) -> Result<NamedStage> {
    let fs = sys().fs;
    info!("mounting {}files under {pwd}", if fs.is_dir(&pwd.join(".git")) { "git " } else { "" });

    let (keep, lose) = {
        let mut entries =
            fs.read_dir(pwd).map_err(|e| anyhow!("Failed reading dir {pwd:?}: {e}"))?;
        entries.sort(); // deterministic iteration
        entries.into_iter().partition(|fname| {
            if fname == ".dockerignore" {
                debug!("excluding {fname}");
                return false;
            }
            if is_named_same_as_virtual_target_dir(fname) {
                debug!("excluding {fname} or it will clash with internal target dir");
                return false;
            }
            if fname == ".git" && fs.is_dir(&pwd.join(fname)) {
                debug!("excluding {fname} dir");
                return false; // Skip copying .git dir
            }
            if fs.exists(&pwd.join(fname).join("CACHEDIR.TAG")) {
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
