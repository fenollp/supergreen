use anyhow::{anyhow, Result};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info};

use crate::stage::{Stage, RST};

pub(crate) const CRATESIO_INDEX: &str = "index.crates.io-0000000000000000";

#[must_use]
pub(crate) fn rewrite_cratesio_index(path: &Utf8Path) -> Utf8PathBuf {
    let prefix = CRATESIO_INDEX.trim_end_matches('0');
    path.iter().map(|part| if part.starts_with(prefix) { CRATESIO_INDEX } else { part }).collect()
}

pub(crate) async fn into_stage(
    cargo_home: &Utf8Path,
    name: &str,
    version: &str,
    krate_manifest_dir: &Utf8Path,
) -> Result<(Stage, &'static str, Utf8PathBuf, String)> {
    let stage = Stage::cratesio(name, version)?;

    let cratesio_extracted =
        cargo_home.join(format!("registry/src/{CRATESIO_INDEX}/{name}-{version}"));
    let cratesio_cached = {
        // e.g. CARGO_MANIFEST_DIR="$CARGO_HOME/registry/src/index.crates.io-1949cf8c6b5b557f/pico-args-0.5.0"
        let cratesio_index = krate_manifest_dir.parent().unwrap().file_name().unwrap();
        cargo_home.join(format!("registry/cache/{cratesio_index}/{name}-{version}.crate"))
    };

    info!("opening (RO) crate tarball {cratesio_cached}");
    let cratesio_hash = sha256::try_async_digest(&cratesio_cached) //TODO: read from lockfile? cargo_metadata?
        .await
        .map_err(|e| anyhow!("Failed reading {cratesio_cached}: {e}"))?;
    debug!("crate sha256 for {stage}: {cratesio_hash}");

    const SRC: &str = "/extracted";

    let add = add_step(name, version, &cratesio_hash);

    // On using tar: https://github.com/rust-lang/cargo/issues/3577#issuecomment-890693359

    //TODO: move extracting to lower stage
    let block = format!(
        r#"
FROM scratch AS {stage}
{add}
SHELL ["/usr/bin/dash", "-eux", "-c"]
RUN \
  --mount=from={RST},src=/lib,dst=/lib \
  --mount=from={RST},src=/lib64,dst=/lib64 \
  --mount=from={RST},src=/usr,dst=/usr \
    mkdir {SRC} \
 && tar zxf /crate --strip-components=1 -C {SRC}
"#,
        add = add.trim(),
    );

    Ok((stage, SRC, cratesio_extracted, block))
}

// https://github.com/moby/buildkit/pull/5991#issuecomment-2910103519

#[must_use]
pub(crate) fn add_step(name: &str, version: &str, hash: &str) -> String {
    format!(
        r#"
ADD --chmod=0664 --unpack=true --checksum=sha256:{hash} \
  https://static.crates.io/crates/{name}/{name}-{version}.crate /crate
"#
    )
}
