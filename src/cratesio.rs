use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};

use crate::{Stage, ALPINE};

pub(crate) const CRATESIO_PREFIX: &str = "index.crates.io-";

#[inline]
pub(crate) fn from_cratesio_input_path(input: &Utf8PathBuf) -> Result<(String, String, String)> {
    let mut it = input.iter();
    let mut cratesio_index = String::new();
    let mut cratesio_crate = None;
    while let Some(part) = it.next() {
        if part.starts_with(CRATESIO_PREFIX) {
            cratesio_index = part.to_owned();
            cratesio_crate = it.next();
            break;
        }
    }
    if cratesio_index.is_empty() || cratesio_crate.map_or(true, str::is_empty) {
        bail!("Unexpected cratesio crate path: {input}")
    }
    let cratesio_crate = cratesio_crate.expect("just checked above");

    let Some((name, version)) = cratesio_crate.rsplit_once('-') else {
        bail!("Unexpected cratesio crate format: {cratesio_crate}")
    };

    Ok((name.to_owned(), version.to_owned(), cratesio_index))
}

pub(crate) async fn into_stage(
    krate: &str,
    cargo_home: impl AsRef<Utf8Path>,
    name: &str,
    version: &str,
    cratesio_index: &str,
) -> Result<(Stage, Utf8PathBuf, String)> {
    let cargo_home = cargo_home.as_ref();

    // TODO: see if {cratesio_index} can be dropped from paths (+ stage names) => content hashing + remap-path-prefix?
    let cratesio_stage = Stage::new(format!("{name}-{version}-{cratesio_index}"))?; // No need for more, e.g. crate_type

    let cratesio_extracted =
        cargo_home.join(format!("registry/src/{cratesio_index}/{name}-{version}"));
    let cratesio_cached =
        cargo_home.join(format!("registry/cache/{cratesio_index}/{name}-{version}.crate"));

    log::info!(target:&krate, "opening (RO) crate tarball {cratesio_cached}");
    let cratesio_hash = sha256::try_async_digest(cratesio_cached.as_path())
        .await
        .map_err(|e| anyhow!("Failed reading {cratesio_cached}: {e}"))?;

    const CRATESIO: &str = "https://static.crates.io";
    let mut block = String::new();
    block.push_str(&format!("FROM {ALPINE} AS {cratesio_stage}\n"));
    block.push_str(&format!("ADD --chmod=0664 --checksum=sha256:{cratesio_hash} \\\n"));
    block.push_str(&format!("  {CRATESIO}/crates/{name}/{name}-{version}.crate /crate\n"));
    // Using tar: https://github.com/rust-lang/cargo/issues/3577#issuecomment-890693359
    block.push_str("RUN set -eux && tar -zxf /crate --strip-components=1 -C /tmp/\n");

    Ok((cratesio_stage, cratesio_extracted, block))
}
