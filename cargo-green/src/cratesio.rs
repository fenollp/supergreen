use anyhow::{anyhow, Result};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info};
use serde::{Deserialize, Serialize};

use crate::stage::{AsBlock, AsStage, NamedStage, Stage};

#[must_use]
pub(crate) fn rewrite_cratesio_index(path: &Utf8Path) -> Utf8PathBuf {
    const CRATESIO_INDEX: &str = "index.crates.io-0000000000000000";

    let prefix = CRATESIO_INDEX.trim_end_matches('0');
    path.iter().map(|part| if part.starts_with(prefix) { CRATESIO_INDEX } else { part }).collect()
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
    name: &'a str,
    krate_manifest_dir: &'a Utf8Path,
) -> Result<NamedStage> {
    let name_dash_version = krate_manifest_dir.file_name().unwrap();
    let stage = Stage::cratesio(name_dash_version)?;

    let extracted = rewrite_cratesio_index(krate_manifest_dir);
    let cached = krate_manifest_dir.to_string() + ".crate";
    let cached = cached.replace("/registry/src/", "/registry/cache/");

    info!("opening (RO) crate tarball {cached}");
    let hash = sha256::try_async_digest(&cached) //TODO: read from lockfile, see cargo_green::prebuild()
        .await
        .map_err(|e| anyhow!("Failed reading {cached}: {e}"))?;
    debug!("crate sha256 for {stage}: {hash}");

    Ok(NamedStage::Cratesio(Cratesio {
        stage,
        extracted,
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
