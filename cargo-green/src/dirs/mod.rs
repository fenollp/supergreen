use std::{env, fs, os::unix::fs::MetadataExt};

use anyhow::{Result, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::{PKG, wrap::pass_env};

mod cargo_home;
mod cross;
mod paths;
mod replacing;
mod sentinel;
mod target_dir;

pub(crate) use cargo_home::*;
pub(crate) use cross::*;
pub(crate) use paths::*;
pub(crate) use replacing::*;
pub(crate) use target_dir::*;

pub(crate) fn tmp() -> Utf8PathBuf {
    env::temp_dir().try_into().expect("$TMPDIR is not utf-8")
}

pub(crate) fn pwd() -> Utf8PathBuf {
    env::current_dir()
        .expect("$PWD does not exist or is otherwise unreadable")
        .try_into()
        .expect("$PWD is not utf-8")
}

pub(crate) fn hash(string: &str) -> String {
    let h = format!("{:#x}", crc32fast::hash(string.as_bytes())); //~ 0x..
    h["0x".len()..].to_owned()
}

pub(crate) fn hashed_args() -> String {
    fn keep(k: &str) -> bool {
        let (pass, skip, _) = pass_env(k);
        pass && !skip
    }
    let envs = env::vars().filter_map(|(k, _)| keep(&k).then_some(k)).collect::<Vec<_>>().join(" ");
    let args = env::args().collect::<Vec<_>>().join(" ");
    format!("{}{}", hash(&envs), hash(&args))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Dirs {
    /// A place to write files before atomically moving them
    #[doc(hidden)]
    pub(crate) tmp: Utf8PathBuf,

    /// A place for build result tarballs (containing .rmeta, .rlib, ...)
    #[doc(hidden)]
    pub(crate) results: Utf8PathBuf,

    /// A place for BuildKit stages' cache exports (local cache backend)
    /// <https://docs.docker.com/build/cache/backends/local/>
    #[doc(hidden)]
    pub(crate) buildkit: Utf8PathBuf,
}

pub(crate) fn setup_dirs() -> Result<Option<Dirs>> {
    let Some(xdg) = ProjectDirs::from("", "", PKG) else {
        eprintln!("BUG: no valid $HOME could be retrieved from the OS");
        return Ok(None);
    };

    // Root for all the folders we should ever need: all deletable.
    let app_cache_dir = xdg.cache_dir().to_owned();
    let app_cache_dir: Utf8PathBuf =
        app_cache_dir.try_into().map_err(|e| anyhow!("Corrupted app cache dir path: {e}"))?;
    fs::create_dir_all(&app_cache_dir)
        .map_err(|e| anyhow!("Failed to `mkdir -p {app_cache_dir}`: {e}"))?;

    let tmp = pick_same_partition_temp_dir(&app_cache_dir)?;
    fs::create_dir_all(&tmp).map_err(|e| anyhow!("Failed to `mkdir -p {tmp}`: {e}"))?;

    let results = app_cache_dir.join("results");
    fs::create_dir_all(&results).map_err(|e| anyhow!("Failed to `mkdir -p {results}`: {e}"))?;

    let buildkit = app_cache_dir.join("buildkit");
    fs::create_dir_all(&buildkit).map_err(|e| anyhow!("Failed to `mkdir -p {buildkit}`: {e}"))?;

    Ok(Some(Dirs { tmp, results, buildkit }))
}

/// Use a temp dir we know a `rename` works atomically
fn pick_same_partition_temp_dir(app_cache_dir: &Utf8Path) -> Result<Utf8PathBuf> {
    let acd_meta = fs::metadata(app_cache_dir)
        .map_err(|e| anyhow!("Failed to `stat {app_cache_dir}`: {e}"))?;
    let tmp_dir = tmp();
    let tmp_meta =
        fs::metadata(&tmp_dir).map_err(|e| anyhow!("Failed to `stat {tmp_dir:?}`: {e}"))?;

    if acd_meta.dev() == tmp_meta.dev() {
        return Ok(tmp_dir);
    }
    Ok(app_cache_dir.join("tmp"))
}
