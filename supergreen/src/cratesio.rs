use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};

use crate::{base::RUST, stage::Stage};

pub const CRATESIO_STAGE_PREFIX: &str = "cratesio-";

#[inline]
pub fn from_cratesio_input_path(input: &Utf8PathBuf) -> Result<(String, String, String)> {
    let mut it = input.iter();
    let mut cratesio_index = String::new();
    let mut cratesio_crate = None;
    while let Some(part) = it.next() {
        if part.starts_with("index.crates.io-") {
            part.clone_into(&mut cratesio_index);
            cratesio_crate = it.next();
            break;
        }
    }
    if cratesio_index.is_empty() || cratesio_crate.map_or(true, str::is_empty) {
        bail!("Unexpected cratesio crate path: {input}")
    }
    let cratesio_crate = cratesio_crate.expect("just checked above");

    let mut name = String::new();
    let mut version = String::new();
    let mut prev_dash = false;
    for (i, c) in cratesio_crate.chars().enumerate() {
        if prev_dash && c.is_ascii_digit() {
            name = cratesio_crate[..(i - 1/*i is never zero: prev_dash starts false*/)].to_owned();
            version = cratesio_crate[i..].to_owned();
            break;
        }
        prev_dash = c == '-'
    }
    if name.is_empty() || version.is_empty() {
        bail!("Unexpected cratesio crate format: {cratesio_crate}")
    }

    Ok((name, version, cratesio_index))
}

#[test]
fn test_from_cratesio_input_path() {
    assert_eq!(from_cratesio_input_path(
        &"/home/pete/.cargo/registry/src/index.crates.io-6f17d22bba15001f/ring-0.17.8/src/lib.rs"
            .into(),
    ).unwrap(),
    ("ring".to_owned(), "0.17.8".to_owned(), "index.crates.io-6f17d22bba15001f".to_owned()));

    assert_eq!(from_cratesio_input_path(
        &"/home/pete/.cargo/registry/src/index.crates.io-6f17d22bba15001f/hickory-proto-0.25.0-alpha.1/src/lib.rs"
            .into(),
    ).unwrap(),
    ("hickory-proto".to_owned(), "0.25.0-alpha.1".to_owned(), "index.crates.io-6f17d22bba15001f".to_owned()));
}

pub async fn into_stage(
    krate: &str,
    cargo_home: impl AsRef<Utf8Path>,
    name: &str,
    version: &str,
    cratesio_index: &str,
) -> Result<(Stage, &'static str, Utf8PathBuf, String)> {
    let cargo_home = cargo_home.as_ref();

    // TODO: see if {cratesio_index} can be dropped from paths (+ stage names) => content hashing + remap-path-prefix?
    let cratesio_stage =
        Stage::try_new(format!("{CRATESIO_STAGE_PREFIX}{name}-{version}-{cratesio_index}"))?;

    let cratesio_extracted =
        cargo_home.join(format!("registry/src/{cratesio_index}/{name}-{version}"));
    let cratesio_cached =
        cargo_home.join(format!("registry/cache/{cratesio_index}/{name}-{version}.crate"));

    log::info!(target: &krate, "opening (RO) crate tarball {cratesio_cached}");
    let cratesio_hash = sha256::try_async_digest(cratesio_cached.as_path()) //TODO: read from lockfile? cargo_metadata?
        .await
        .map_err(|e| anyhow!("Failed reading {cratesio_cached}: {e}"))?;
    log::debug!(target: &krate, "crate sha256 for {cratesio_stage}: {cratesio_hash}");

    const CRATESIO: &str = "https://static.crates.io";
    const SRC: &str = "/extracted";

    // On using tar: https://github.com/rust-lang/cargo/issues/3577#issuecomment-890693359

    let block = format!(
        r#"
FROM scratch AS {cratesio_stage}
ADD --chmod=0664 --checksum=sha256:{cratesio_hash} \
  {CRATESIO}/crates/{name}/{name}-{version}.crate /crate
SHELL ["/usr/bin/dash", "-c"]
RUN \
  --mount=from={RUST},src=/lib,dst=/lib \
  --mount=from={RUST},src=/lib64,dst=/lib64 \
  --mount=from={RUST},src=/usr,dst=/usr \
    set -eux \
 && mkdir -p {SRC} \
 && tar zxvf /crate --strip-components=1 -C {SRC}
"#
    )[1..]
        .to_owned();

    // TODO: ask upstream `buildx/buildkit+podman` for a way to drop that RUN
    //  => https://github.com/moby/buildkit/issues/4907

    // Otherwise:

    // TODO: ask upstream `rustc` if it could be able to take a .crate archive as input
    //=> would remove that `RUN tar` step + stage dep on RUST (=> scratch)

    Ok((cratesio_stage, SRC, cratesio_extracted, block))
}
