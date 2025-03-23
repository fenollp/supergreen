use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info};

use crate::{base::RUST, stage::Stage};

pub(crate) const CRATESIO_STAGE_PREFIX: &str = "cratesio-";
pub(crate) const CRATESIO_INDEX: &str = "index.crates.io-0000000000000000";

pub(crate) fn from_cratesio_input_path(input: &Utf8PathBuf) -> Result<(String, String)> {
    let mut it = input.iter();
    let mut cratesio_crate = None;
    while let Some(part) = it.next() {
        if part.starts_with("index.crates.io-") {
            cratesio_crate = it.next();
            break;
        }
    }
    if cratesio_crate.is_none_or(str::is_empty) {
        bail!("Unexpected cratesio crate path: {input}")
    }
    let cratesio_crate = cratesio_crate.expect("just checked above");

    let lhs = cratesio_crate.split_once('+').map(|(l, _)| l).unwrap_or(cratesio_crate); //https://semver.org/#spec-item-10

    let (mut name, mut version) = (String::new(), String::new());
    if let Some(mid) = "1234567890".chars().filter_map(|x| lhs.rfind(&format!("-{x}"))).max() {
        let (n, v) = cratesio_crate.split_at(mid);
        (name, version) = (n.to_owned(), v[1..].to_owned());
    }
    if name.is_empty() || version.is_empty() {
        bail!("Unexpected cratesio crate name-version format: {cratesio_crate}")
    }

    Ok((name, version))
}

#[test]
fn test_from_cratesio_input_path() {
    assert_eq!(from_cratesio_input_path(
        &"/home/pete/.cargo/registry/src/index.crates.io-6f17d22bba15001f/ring-0.17.8/src/lib.rs"
            .into(),
    ).unwrap(),
    ("ring".to_owned(), "0.17.8".to_owned()));

    assert_eq!(from_cratesio_input_path(
        &"/home/pete/.cargo/registry/src/index.crates.io-6f17d22bba15001f/hickory-proto-0.25.0-alpha.1/src/lib.rs"
            .into(),
    ).unwrap(),
    ("hickory-proto".to_owned(), "0.25.0-alpha.1".to_owned()));

    assert_eq!(from_cratesio_input_path(
        &"/home/pete/.cargo/registry/src/index.crates.io-6f17d22bba15001f/md-5-0.10.6/src/lib.rs"
            .into(),
    ).unwrap(),
    ("md-5".to_owned(), "0.10.6".to_owned()));

    assert_eq!(from_cratesio_input_path(
        &"/home/pete/.cargo/registry/src/index.crates.io-6f17d22bba15001f/curl-sys-0.4.74+curl-8.9.0/lib.rs"
            .into(),
    ).unwrap(),
    ("curl-sys".to_owned(), "0.4.74+curl-8.9.0".to_owned()));

    // /home/pete/.cargo/registry/cache/index.crates.io-6f17d22bba15001f/curl-sys-0.4.74+curl-8.9.0.crate
}

#[must_use]
pub(crate) fn rewrite_cratesio_index(path: &Utf8Path) -> Utf8PathBuf {
    let prefix = CRATESIO_INDEX.trim_end_matches('0');
    path.iter().map(|part| if part.starts_with(prefix) { CRATESIO_INDEX } else { part }).collect()
}

pub(crate) async fn into_stage(
    cargo_home: &Utf8Path,
    name: &str,
    version: &str,
) -> Result<(Stage, &'static str, Utf8PathBuf, String)> {
    let cratesio_stage = Stage::try_new(format!("{CRATESIO_STAGE_PREFIX}{name}-{version}"))?;

    let cratesio_extracted =
        cargo_home.join(format!("registry/src/{CRATESIO_INDEX}/{name}-{version}"));
    let cratesio_cached = {
        // e.g. CARGO_MANIFEST_DIR="$CARGO_HOME/registry/src/index.crates.io-1949cf8c6b5b557f/pico-args-0.5.0"
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").ok().unwrap();
        let manifest_dir = Utf8Path::new(&manifest_dir);
        let cratesio_index = manifest_dir.parent().unwrap().file_name().unwrap();
        cargo_home.join(format!("registry/cache/{cratesio_index}/{name}-{version}.crate"))
    };

    info!("opening (RO) crate tarball {cratesio_cached}");
    let cratesio_hash = sha256::try_async_digest(cratesio_cached.as_path()) //TODO: read from lockfile? cargo_metadata?
        .await
        .map_err(|e| anyhow!("Failed reading {cratesio_cached}: {e}"))?;
    debug!("crate sha256 for {cratesio_stage}: {cratesio_hash}");

    const CRATESIO: &str = "https://static.crates.io";
    const SRC: &str = "/extracted";

    // On using tar: https://github.com/rust-lang/cargo/issues/3577#issuecomment-890693359

    let block = format!(
        r#"
FROM scratch AS {cratesio_stage}
ADD --chmod=0664 --checksum=sha256:{cratesio_hash} \
  {CRATESIO}/crates/{name}/{name}-{version}.crate /crate
SHELL ["/usr/bin/dash", "-eux", "-c"]
RUN \
  --mount=from={RUST},src=/lib,dst=/lib \
  --mount=from={RUST},src=/lib64,dst=/lib64 \
  --mount=from={RUST},src=/usr,dst=/usr \
    mkdir {SRC} \
 && tar zxf /crate --strip-components=1 -C {SRC}
"#
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
