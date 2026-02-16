use std::{fs, iter::once};

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info};
use serde::{Deserialize, Serialize};

use crate::{
    md::MdId,
    stage::{AsBlock, AsStage, NamedStage, Stage},
    target_dir::VIRTUAL_TARGET_DIR,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub(crate) struct Relative {
    stage: Stage,
    pwd: Utf8PathBuf,
    keep: Vec<String>,
    lose: Vec<String>,
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

    fn context(&self) -> Option<(Stage, Utf8PathBuf)> {
        let Self { stage, lose, pwd, .. } = self;
        if !lose.is_empty() {
            let mut lose: Vec<String> = lose
                .iter()
                .chain(once(&".dockerignore".to_owned()))
                .map(|fname| format!("/{fname}\n"))
                .collect();
            lose.sort();
            lose.dedup();
            let lose: String = lose.into_iter().collect();
            fs::write(pwd.join(".dockerignore"), lose).unwrap(); //TODO: remove created file
                                                                 //FIXME: if exists: save + extend (then restore??) .dockerignore
                                                                 //TODO? add .gitignore in there?
                                                                 //TODO? exclude everything, only include `git ls-files`?
        }
        Some((stage.to_owned(), pwd.to_owned()))
    }
}

/// NOTE: build contexts have to be directories, can't be files.
///> failed to get build context path {$HOME/wefwefwef/supergreen.git/Cargo.lock <nil>}: not a directory
pub(crate) async fn as_stage(mdid: MdId, pwd: &Utf8Path) -> Result<NamedStage> {
    info!("mounting {}files under {pwd}", if pwd.join(".git").is_dir() { "git " } else { "" });

    let (keep, lose) = {
        let mut entries = fs::read_dir(pwd)
            .map_err(|e| anyhow!("Failed reading dir {pwd:?}: {e}"))?
            .map(|entry| -> Result<_> {
                let entry = entry?;
                let fpath = entry.path();
                let fpath: Utf8PathBuf = fpath
                    .try_into()
                    .map_err(|e| anyhow!("corrupted UTF-8 encoding with {entry:?}: {e}"))?;
                let Some(fname) = fpath.file_name() else {
                    bail!("unexpected root (/) for {entry:?}")
                };
                Ok(fname.to_owned())
            })
            .collect::<Result<Vec<_>>>()?;
        entries.sort(); // deterministic iteration
        entries.into_iter().partition(|fname| {
            if fname == ".dockerignore" {
                debug!("excluding {fname}");
                return false;
            }
            if fname == VIRTUAL_TARGET_DIR.trim_matches('/') {
                debug!("excluding {fname} or it will clash with internal target dir");
                return false;
            }
            if fname == ".git" && pwd.join(fname).is_dir() {
                debug!("excluding {fname} dir");
                return false; // Skip copying .git dir
            }
            if pwd.join(fname).join("CACHEDIR.TAG").exists() {
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
    }))
}
