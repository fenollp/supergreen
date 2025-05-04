use std::sync::OnceLock;

use anyhow::{anyhow, Result};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info};

use crate::stage::{Stage, RST};

pub(crate) const VIRTUAL_INDEX_PART: &str = "index.crates.io-0000000000000000";

static INDEX_PART: OnceLock<String> = OnceLock::new(); //FIXME: in Md, because reused.

#[cfg(test)]
pub(crate) fn set_some_index_part() {
    let _ = INDEX_PART.get_or_init(|| "index.crates.io-0123456789abcdef".to_owned());
}

/// Sets `INDEX_PART` on first call for the duration of the $RUSTC_WRAPPER process.
#[must_use]
pub(crate) fn hide_cratesio_index(path: &Utf8Path) -> Utf8PathBuf {
    let prefix = VIRTUAL_INDEX_PART.trim_end_matches('0');
    path.iter()
        .map(|part| {
            if part.starts_with(prefix) {
                let _ = INDEX_PART.get_or_init(|| part.to_owned());

                VIRTUAL_INDEX_PART
            } else {
                part
            }
        })
        .collect()
}

#[must_use]
pub(crate) fn unhide_cratesio_index(txt: &str) -> String {
    if let Some(actual) = INDEX_PART.get() {
        return txt.replace(VIRTUAL_INDEX_PART, actual);
    }
    txt.to_owned()
}

#[must_use]
pub(crate) fn unhide_cratesio_index_bytes(txt: &[u8]) -> Vec<u8> {
    use bstr::ByteSlice;
    if let Some(actual) = INDEX_PART.get() {
        return txt.replace(VIRTUAL_INDEX_PART, actual);
    }
    txt.to_owned()
}

pub(crate) async fn into_stage(
    cargo_home: &Utf8Path,
    name: &str,
    version: &str,
    krate_manifest_dir: &Utf8Path,
) -> Result<(Stage, &'static str, Utf8PathBuf, String)> {
    let stage = Stage::cratesio(name, version)?;

    let cratesio_extracted =
        cargo_home.join(format!("registry/src/{VIRTUAL_INDEX_PART}/{name}-{version}"));
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

    // TODO: ask upstream `buildx/buildkit+podman` for a way to drop that RUN
    //  => TODO: impl --unpack: https://github.com/moby/buildkit/issues/4907

    // Otherwise:

    // TODO: ask upstream `rustc` if it could be able to take a .crate archive as input
    //=> would remove that `RUN tar` step + stage dep on RUST (=> scratch)
    //  => https://github.com/rust-lang/cargo/issues/14373

    Ok((stage, SRC, cratesio_extracted, block))
}

#[must_use]
pub(crate) fn add_step(name: &str, version: &str, hash: &str) -> String {
    format!(
        r#"
ADD --chmod=0664 --checksum=sha256:{hash} \
  https://static.crates.io/crates/{name}/{name}-{version}.crate /crate
"#
    )
}
