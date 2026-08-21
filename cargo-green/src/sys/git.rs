use anyhow::{Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};

/// Git repository discovery, for crates checked out under `$CARGO_HOME/git/checkouts`.
pub(crate) trait Git: Send + Sync {
    /// Locate the `FETCH_HEAD` of the git db backing `pkg_manifest_dir`'s checkout.
    ///
    /// e.g. `$CARGO_HOME/git/db/remarkable-tools-9f4e9942cc4e93a3/FETCH_HEAD`
    fn fetch_head(&self, pkg_manifest_dir: &Utf8Path) -> Result<Utf8PathBuf>;
}

pub(crate) struct RealGit;

impl Git for RealGit {
    fn fetch_head(&self, pkg_manifest_dir: &Utf8Path) -> Result<Utf8PathBuf> {
        use gix_config::{File, Source};

        // let config_path = pkg_manifest_dir.join(".git/config");
        // e.g.: CARGO_MANIFEST_DIR="$CARGO_HOME/git/checkouts/cross-f0189a1dc141e2d9/88f49ff"
        let (path, _trust) =
            gix_discover::upwards(pkg_manifest_dir.as_std_path()).map_err(|e| {
                anyhow!("Failed getting repository directoy from {pkg_manifest_dir}: {e}")
            })?;
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
}
