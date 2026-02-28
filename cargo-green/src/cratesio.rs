use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info};
use serde::{Deserialize, Serialize};

use crate::{
    base_image::rewrite_cargo_home,
    green::Green,
    stage::{AsBlock, AsStage, NamedStage, Stage},
};

pub(crate) const HOME: &str = "registry/src";

const INDEX: &str = "index.crates.io";

impl Green {
    pub(crate) fn maybe_arrange_cratesio_index(&self) -> Result<()> {
        let crates_home = self.cargo_home.join(HOME);
        info!("Listing directory {crates_home}");
        if let Some(youngest) = crates_home
            .read_dir_utf8()
            .map_err(|e| anyhow!("Failed `ls {crates_home}`: {e}"))?
            .filter_map(Result::ok)
            .inspect(|entry| info!("Found {}: {:?}", entry.path(), entry.file_type()))
            .filter(|entry| entry.file_type().map(|f| f.is_dir()).unwrap_or(false))
            .filter(|dir| dir.path().as_str().starts_with(INDEX))
            .filter(|dir| dir.path().file_name() != Some(INDEX)) // Just to be sure
            .filter_map(|dir| Some((dir.path().to_owned(), dir.metadata().ok()?.modified().ok()?)))
            .max_by_key(|&(_, modified)| modified)
            .map(|(path, _)| path)
        {
            let link = youngest.with_file_name(INDEX);
            if let Err(e) = symlink::remove_symlink_dir(&link) {
                info!("Failed cleaning previous symlink {link}: {e}");
            }
            if let Err(e) = symlink::symlink_dir(&link, &youngest) {
                bail!("Could not symlink {link} to {youngest}: {e}")
            }
        }
        Ok(())
    }
}

#[must_use]
pub(crate) fn rewrite_cratesio_index(path: &str) -> String {
    if let Some(pos) = path.find(INDEX) {
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
    cargo_home: &Utf8Path,
    name: &'a str,
    krate_manifest_dir: &'a Utf8Path,
) -> Result<NamedStage> {
    let name_dash_version = krate_manifest_dir.file_name().unwrap();
    let stage = Stage::cratesio(name_dash_version)?;

    let cached = krate_manifest_dir.to_string() + ".crate";
    let cached = cached.replace(&format!("/{HOME}/"), "/registry/cache/");

    info!("opening (RO) crate tarball {cached}");
    let hash = sha256::try_async_digest(&cached) //TODO: read from lockfile, see cargo_green::prebuild()
        .await
        .map_err(|e| anyhow!("Failed reading {cached}: {e}"))?;
    debug!("crate sha256 for {stage}: {hash}");

    let krate_manifest_dir = rewrite_cargo_home(cargo_home, krate_manifest_dir.as_str());
    let extracted = rewrite_cratesio_index(&krate_manifest_dir);

    Ok(NamedStage::Cratesio(Cratesio {
        stage,
        extracted: extracted.into(),
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
ADD --chmod=0664 --unpack --checksum=sha256:{hash} \
  https://static.crates.io/crates/{name}/{name_dash_version}.crate /
"#
    )
}
