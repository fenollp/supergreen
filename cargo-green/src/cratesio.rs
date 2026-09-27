use anyhow::{Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info};
use serde::{Deserialize, Serialize};

use crate::{
    dirs::Paths,
    stage::{AsBlock, AsStage, NamedStage, Stage},
    sys::fs,
};

const HOME: &str = "registry/src";

const INDEX: &str = "index.crates.io";

impl Paths {
    pub(crate) fn maybe_arrange_cratesio_index(&self) -> Result<()> {
        let crates_home = self.cratesio_home();
        info!("Listing directory {crates_home}");
        if !crates_home.exists() {
            info!("making usre {crates_home} exists...");
            // No root rights needed here
            fs().create_dir_all(&crates_home)
                .map_err(|e| anyhow!("Failed to `mkdir -p {crates_home}`: {e}"))?;
        }
        if let Some(youngest) = fs()
            .read_dir(&crates_home)
            .map_err(|e| anyhow!("Failed `ls {crates_home}`: {e}"))?
            .into_iter()
            .inspect(|p| {
                info!("Found {p}: (dir,reg,sym) = {:?}", (p.is_dir(), p.is_file(), p.is_symlink()))
            })
            .filter(|p| p.is_dir())
            .filter(|dir| {
                dir.file_name()
                    .map(|name: &str| name.starts_with(INDEX) && name != INDEX)
                    .unwrap_or(false)
            })
            .filter_map(|dir| Some((dir.metadata().ok()?.modified().ok()?, dir)))
            .max_by_key(|&(modified, _)| modified)
            .map(|(_, path)| path)
        {
            let link = youngest.with_file_name(INDEX);
            if let Err(e) = symlink::remove_symlink_dir(&link) {
                info!("Failed cleaning previous symlink {link}: {e}");
            }
            if let Err(e) = symlink::symlink_dir(&youngest, &link) {
                bail!("Could not symlink {link} to {youngest}: {e}")
            }
        }
        Ok(())
    }
}

impl Paths {
    #[must_use]
    pub(crate) fn cratesio_home(&self) -> Utf8PathBuf {
        self.cargo_home.join(HOME)
    }

    #[must_use]
    pub(crate) fn is_cratesio(&self, path: &Utf8Path) -> bool {
        path.starts_with(self.cratesio_home())
    }
}

#[must_use]
pub(crate) fn rewrite_cratesio_index(path: &str) -> String {
    if let Some(pos) = path.find(&format!("{INDEX}-")) {
        return path[..pos].to_owned() + INDEX + &path[(pos + INDEX.len() + 1 + 16)..];
    }
    path.to_owned()
}

#[test]
fn test_rewrite_cratesio_index() {
    assert_eq!(
        format!("$CARGO_HOME/{HOME}/index.crates.io/anyhow-1.0.100"),
        rewrite_cratesio_index(&format!(
            "$CARGO_HOME/{HOME}/index.crates.io-f9fd03f8c3c43dd1/anyhow-1.0.100"
        ))
    );
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub(crate) struct Cratesio {
    stage: Stage,
    extracted: Utf8PathBuf,
    name: String,
    name_dash_version: String,
    hash: String,
}

impl AsBlock for Cratesio {
    fn as_block(&self) -> Option<String> {
        let Self { stage, name, name_dash_version, hash, .. } = self;
        let add = add_step(name, name_dash_version, hash);
        Some(format!(
            r#"
FROM scratch AS {stage}
{add}
"#,
            add = add.trim(),
        ))
    }
}

impl AsStage<'_> for Cratesio {
    fn name(&self) -> &Stage {
        &self.stage
    }

    fn mounts(&self) -> Vec<(Option<Utf8PathBuf>, Utf8PathBuf, bool)> {
        let Self { extracted, name_dash_version, .. } = self;
        vec![(Some(format!("/{name_dash_version}").into()), extracted.clone(), false)]
    }
}

/// CARGO_MANIFEST_DIR="$CARGO_HOME/registry/src/index.crates.io-1949cf8c6b5b557f/pico-args-0.5.0"
pub(crate) async fn named_stage<'a>(
    paths: &Paths,
    name: &'a str,
    pkg_manifest_dir: &'a Utf8Path,
) -> Result<NamedStage> {
    let Some(name_dash_version) = pkg_manifest_dir.file_name() else {
        bail!("BUG: malformed pkg_manifest_dir: {pkg_manifest_dir}")
    };
    let stage = Stage::cratesio(name_dash_version)?;

    let cached = pkg_manifest_dir.to_string() + ".crate";
    let cached = cached.replace(&format!("/{HOME}/"), "/registry/cache/");

    info!("opening (RO) crate tarball {cached}");
    let hash = fs()
        .sha256(Utf8Path::new(&cached)) //TODO: read from lockfile, see cargo_green::prebuild()
        .await
        .map_err(|e| anyhow!("Failed reading {cached}: {e}"))?;
    debug!("crate sha256 for {stage}: {hash}");

    Ok(NamedStage::Cratesio(Cratesio {
        stage,
        extracted: paths.rewrite(pkg_manifest_dir).into(),
        name: name.to_owned(),
        name_dash_version: name_dash_version.to_owned(),
        hash,
    }))
}

// [Consider making the src cache read-only](https://github.com/rust-lang/cargo/issues/9455)
#[must_use]
pub(crate) fn add_step(name: &str, name_dash_version: &str, hash: &str) -> String {
    format!(
        r#"
ADD --unpack --checksum=sha256:{hash} \
  https://static.crates.io/crates/{name}/{name_dash_version}.crate /
"#
    )
}

/// A crates.io dependency becomes an `ADD` of its tarball, pinned by the checksum of
/// the copy cargo already downloaded, and mounted where cargo unpacked it.
#[cfg(test)]
mod as_stage {
    use super::{Paths, named_stage};
    use crate::sys::{Sys, fake::FakeFs};

    const INDEX: &str = "index.crates.io-1949cf8c6b5b557f";

    fn describe_crate(name: &str, name_dash_version: &str) -> String {
        let fs = FakeFs::new();
        fs.file(format!("/home/u/.cargo/registry/cache/{INDEX}/{name_dash_version}.crate"), "");
        let _guard = Sys::install(Sys { fs, ..Sys::fake() });

        let paths = Paths {
            cargo_home: "/home/u/.cargo".into(),
            cwd: "/home/u/mycrate".into(),
            host_target_dir: Some("/home/u/mycrate/target".into()),
            ..Default::default()
        };
        let manifest_dir = format!("/home/u/.cargo/registry/src/{INDEX}/{name_dash_version}");
        let ns = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(named_stage(&paths, name, manifest_dir.as_str().into()))
            .unwrap();
        ns.describe()
    }

    /// The index's hashed dir name is host-specific: it is dropped from the mount.
    #[test]
    fn a_registry_crate() {
        assert_snapshots_eq!(
            describe_crate("pico-args", "pico-args-0.5.0"),
            snapbox::str![[r#"
[Cratesio]
stage = "cratesio-pico-args-0.5.0"
extracted = "$CARGO_HOME/registry/src/index.crates.io/pico-args-0.5.0"
name = "pico-args"
name_dash_version = "pico-args-0.5.0"
hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"

# as_block
FROM scratch AS cratesio-pico-args-0.5.0
ADD --unpack --checksum=sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 \
  https://static.crates.io/crates/pico-args/pico-args-0.5.0.crate /

# mounts
/pico-args-0.5.0 -> $CARGO_HOME/registry/src/index.crates.io/pico-args-0.5.0

"#]]
        );
    }

    /// Semver build metadata (`+…`) is not allowed in a stage name but is kept
    /// everywhere else. (corpus: `bzip2-sys-0.1.11+1.0.8`, `cargo-c-0.10.18+cargo-0.92.0`)
    #[test]
    fn a_version_with_build_metadata() {
        assert_snapshots_eq!(
            describe_crate("bzip2-sys", "bzip2-sys-0.1.11+1.0.8"),
            snapbox::str![[r#"
[Cratesio]
stage = "cratesio-bzip2-sys-0.1.11-1.0.8"
extracted = "$CARGO_HOME/registry/src/index.crates.io/bzip2-sys-0.1.11+1.0.8"
name = "bzip2-sys"
name_dash_version = "bzip2-sys-0.1.11+1.0.8"
hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"

# as_block
FROM scratch AS cratesio-bzip2-sys-0.1.11-1.0.8
ADD --unpack --checksum=sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 \
  https://static.crates.io/crates/bzip2-sys/bzip2-sys-0.1.11+1.0.8.crate /

# mounts
/bzip2-sys-0.1.11+1.0.8 -> $CARGO_HOME/registry/src/index.crates.io/bzip2-sys-0.1.11+1.0.8

"#]]
        );
    }
}
