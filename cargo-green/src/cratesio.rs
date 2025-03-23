use anyhow::{anyhow, Result};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info};

use crate::{base::RUST, stage::Stage};

pub(crate) const CRATESIO_STAGE_PREFIX: &str = "cratesio-";
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
    let cratesio_stage = Stage::try_new(format!("{CRATESIO_STAGE_PREFIX}{name}-{version}"))?;

    let cratesio_extracted =
        cargo_home.join(format!("registry/src/{CRATESIO_INDEX}/{name}-{version}"));
    let cratesio_cached = {
        // e.g. CARGO_MANIFEST_DIR="$CARGO_HOME/registry/src/index.crates.io-1949cf8c6b5b557f/pico-args-0.5.0"
        let cratesio_index = krate_manifest_dir.parent().unwrap().file_name().unwrap();
        cargo_home.join(format!("registry/cache/{cratesio_index}/{name}-{version}.crate"))
    };

    info!("opening (RO) crate tarball {cratesio_cached}");
    let cratesio_hash = sha256::try_async_digest(cratesio_cached.as_path()) //TODO: read from lockfile? cargo_metadata?
        .await
        .map_err(|e| anyhow!("Failed reading {cratesio_cached}: {e}"))?;
    debug!("crate sha256 for {cratesio_stage}: {cratesio_hash}");

    const SRC: &str = "/extracted";

    // On using tar: https://github.com/rust-lang/cargo/issues/3577#issuecomment-890693359

    let block = format!(
        r#"
FROM scratch AS {cratesio_stage}
{add}
SHELL ["/usr/bin/dash", "-eux", "-c"]
RUN \
  --mount=from={RUST},src=/lib,dst=/lib \
  --mount=from={RUST},src=/lib64,dst=/lib64 \
  --mount=from={RUST},src=/usr,dst=/usr \
    mkdir {SRC} \
 && tar zxf /crate --strip-components=1 -C {SRC}
"#,
        add = add_step(name, version, &cratesio_hash),
    )[1..]
        .to_owned();

    // TODO: ask upstream `buildx/buildkit+podman` for a way to drop that RUN
    //  => https://github.com/moby/buildkit/issues/4907

    // Otherwise:

    // TODO: ask upstream `rustc` if it could be able to take a .crate archive as input
    //=> would remove that `RUN tar` step + stage dep on RUST (=> scratch)
    //  => https://github.com/rust-lang/cargo/issues/14373

    Ok((cratesio_stage, SRC, cratesio_extracted, block))
}

#[must_use]
pub(crate) fn add_step(name: &str, version: &str, hash: &str) -> String {
    format!(
        r#"
ADD --chmod=0664 --checksum=sha256:{hash} \
  https://static.crates.io/crates/{name}/{name}-{version}.crate /crate
"#
    )[1..]
        .to_owned()
}
