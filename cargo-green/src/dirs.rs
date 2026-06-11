use std::{env, fs, os::unix::fs::MetadataExt};

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::{green::Green, wrap::pass_env, PKG, VSN};

pub(crate) fn tmp() -> Utf8PathBuf {
    env::temp_dir().try_into().expect("$TMPDIR is not utf-8")
}

pub(crate) fn pwd() -> Utf8PathBuf {
    env::current_dir()
        .expect("$PWD does not exist or is otherwise unreadable")
        .try_into()
        .expect("$PWD is not utf-8")
}

pub(crate) fn cargo_home() -> Result<Utf8PathBuf> {
    home::cargo_home()
        .map_err(|e| anyhow!("Bad $CARGO_HOME or something: {e}"))?
        .try_into()
        .map_err(|e| anyhow!("Corrupted $CARGO_HOME path: {e}"))
}

pub(crate) fn create_current_target_dir(command: Option<&str>) -> Result<String> {
    let target_dir = if let Some(target_dir) = {
        let mut args = pico_args::Arguments::from_env();
        args.opt_value_from_str("--target-dir")?
    } {
        target_dir
    } else if let Ok(target_dir) = env::var("CARGO_TARGET_DIR") {
        target_dir
    } else if false {
        todo!("check build.target-dir in config.toml.s")
    } else if command == Some("install") {
        tmp().join(hashed_args()).to_string()
    } else {
        // TODO: fallback to workspace root, not necessarily pwd()
        pwd().join("target").to_string()
    };

    fs::create_dir_all(&target_dir)?;

    let target_dir = Utf8PathBuf::from(&target_dir)
        .canonicalize_utf8()
        .map_err(|e| anyhow!("Failed to canonicalize target dir {target_dir}: {e}"))?;
    Ok(format!("{target_dir}/")) // Trailing slash required when replacing strings
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
}

impl Green {
    pub(crate) fn setup_dirs(&mut self) -> Result<()> {
        let Some(xdg) = ProjectDirs::from("", "", PKG) else {
            bail!("BUG: no valid $HOME could be retrieved from the OS")
        };

        // Root for all the folders we should ever need: all deletable.
        let app_cache_dir = xdg.cache_dir().to_owned();
        let app_cache_dir: Utf8PathBuf =
            app_cache_dir.try_into().map_err(|e| anyhow!("Corrupted app cache dir path: {e}"))?;
        fs::create_dir_all(&app_cache_dir)
            .map_err(|e| anyhow!("Failed to `mkdir -p {app_cache_dir}`: {e}"))?;

        // A local copy of (remotely) cached results
        let results = app_cache_dir.join("results");
        fs::create_dir_all(&results).map_err(|e| anyhow!("Failed to `mkdir -p {results}`: {e}"))?;

        // TODO: $APPCACHEDIR/buildkit (with compatibility-version=20) using file exporter

        // A /tmp "local" to appcachedir
        let tmp = pick_same_partition_temp_dir(&app_cache_dir)?;
        fs::create_dir_all(&tmp).map_err(|e| anyhow!("Failed to `mkdir -p {tmp}`: {e}"))?;

        self.dirs = Some(Dirs { tmp, results });

        Ok(())
    }
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

impl Green {
    /// Includes builder container ID so its recreation retries builds
    pub(crate) fn sentinel_path(&self, name: &str, ext: &str) -> Utf8PathBuf {
        let builder = self.builder.id.as_deref().map(|id| format!("x{id:.12}")).unwrap_or_default();
        tmp().join(format!("{PKG}v{VSN}{builder}-{name}.{ext}"))
    }
}
