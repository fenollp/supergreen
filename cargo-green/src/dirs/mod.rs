use std::{env, ffi::OsStr, fs, os::unix::fs::PermissionsExt};

use anyhow::{Result, anyhow, bail};
use camino::Utf8PathBuf;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::{
    PKG,
    redact::Vars,
    sys::{VAR_RUN_MODE, fs},
    wrap::pass_env,
};

mod cargo_home;
mod cross;
mod cwd;
mod paths;
mod replacing;
mod sentinel;
mod target_dir;

pub(crate) use cargo_home::*;
pub(crate) use cross::*;
pub(crate) use cwd::*;
pub(crate) use paths::*;
pub(crate) use replacing::*;
pub(crate) use target_dir::*;

pub(crate) fn tmp() -> Utf8PathBuf {
    env::temp_dir().try_into().expect("$TMPDIR is not utf-8")
}

pub(crate) fn hash(string: &str) -> String {
    let h = format!("{:#x}", crc32fast::hash(string.as_bytes())); //~ 0x..
    h["0x".len()..].to_owned()
}

pub(crate) fn hashed_args<'a>(env: &Vars, args: impl Iterator<Item = &'a OsStr>) -> String {
    fn keep(k: &str) -> bool {
        let (pass, skip, _) = pass_env(k);
        pass && !skip
    }
    let envs = env.keys().filter(|&k| keep(k)).map(ToOwned::to_owned).collect::<Vec<_>>().join(" ");
    let args = args.collect::<Vec<_>>().join(OsStr::new(" "));
    format!("{}{}", hash(&envs), hash(&args.to_string_lossy()))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Dirs {
    /// A place for build result tarballs (containing .rmeta, .rlib, ...)
    #[doc(hidden)]
    pub(crate) results: Utf8PathBuf,

    /// A place for BuildKit stages' cache exports (local cache backend)
    /// <https://docs.docker.com/build/cache/backends/local/>
    #[doc(hidden)]
    pub(crate) buildkit: Utf8PathBuf,

    /// A private place for sockets: `$XDG_RUNTIME_DIR/cargo-green` when available
    /// e.g. /run/user/1000: a short, private and tmpfs-backed on Linux.
    #[doc(hidden)]
    pub(crate) var_run: Utf8PathBuf,
}

impl Dirs {
    pub(crate) fn setup(&self) -> Result<()> {
        let Dirs { results, buildkit, var_run } = self;
        for path in [results, buildkit, var_run] {
            if let Err(e) = fs().create_dir_all(path) {
                bail!("Failed to `mkdir -p {path}`: {e}")
            }
        }
        fs::set_permissions(var_run, fs::Permissions::from_mode(VAR_RUN_MODE))
            .map_err(|e| anyhow!("Failed to `chmod 700 {var_run}`: {e}"))
    }
}

/// Where our XDG dirs are to be, without touching them
pub(crate) fn our_dirs() -> Result<Option<Dirs>> {
    let Some(xdg) = ProjectDirs::from("", "", PKG) else {
        // Not returning an error: to help doctoring
        eprintln!("BUG: no valid $HOME could be retrieved from the OS");
        return Ok(None);
    };

    let errf = |what: &'static str| move |e| anyhow!("Corrupted app {what} dir path: {e}");

    // Root for all the folders we should ever need: all deletable.
    let app_cache_dir = xdg.cache_dir().to_owned();
    let app_cache_dir: Utf8PathBuf = app_cache_dir.try_into().map_err(errf("cache"))?;

    let results = app_cache_dir.join("results");
    let buildkit = app_cache_dir.join("buildkit");
    let var_run = match xdg.runtime_dir() {
        None => app_cache_dir.join("run"),
        Some(dir) => dir.to_owned().try_into().map_err(errf("runtime"))?,
    };
    Ok(Some(Dirs { results, buildkit, var_run }))
}
