//! Results as `oci-layout://` build contexts: a result tarball is a valid image layer as is,
//! so an `out-<mdid>` stage's recipe can be swapped for an image made of that one (hard linked) layer.

use std::io::{ErrorKind, Read};

use anyhow::{Result, anyhow};
use camino::Utf8Path;
use ruzstd::decoding::StreamingDecoder;

use crate::{
    dirs::Dirs,
    md::BuildContext,
    stage::Stage,
    sys::{TAR_MODE, fs},
};

/// Holds the digest of the layout's manifest. Written last: its presence means the layout is whole.
const MANIFEST_DIGEST: &str = "manifest";

impl Dirs {
    /// The build context that stands for `stage`, if its result was stored.
    pub(crate) fn result_as_context(&self, stage: &Stage) -> Result<Option<BuildContext>> {
        let result = self.result_from_stage(stage);
        if !fs().exists(&result) {
            return Ok(None);
        }
        let layout = self.results.join("oci").join(stage.bucketed());
        let digest = match fs().read_to_string(&layout.join(MANIFEST_DIGEST)) {
            Ok(digest) => digest,
            Err(e) if e.kind() == ErrorKind::NotFound => write_layout(&result, &layout)?,
            Err(e) => return Err(anyhow!("Failed reading OCI layout {layout}: {e}")),
        };
        let uri = format!("oci-layout://{layout}@sha256:{digest}").into();
        Ok(Some(BuildContext { name: stage.clone(), uri }))
    }
}

/// Returns the manifest's digest.
fn write_layout(result: &Utf8Path, layout: &Utf8Path) -> Result<String> {
    let errf = |e| anyhow!("Failed writing OCI layout {layout} of {result}: {e}");

    let mut layer = vec![];
    let _ = fs().open(result).map_err(errf)?.read_to_end(&mut layer).map_err(errf)?;
    let mut tar = vec![];
    let _ = StreamingDecoder::new(layer.as_slice())
        .map_err(|e| anyhow!("Failed decoding {result}: {e}"))?
        .read_to_end(&mut tar)
        .map_err(errf)?;

    let blobs = layout.join("blobs/sha256");
    fs().create_dir_all(&blobs).map_err(errf)?;
    let blob = |data: &str| -> Result<String> {
        let digest = sha256::digest(data);
        fs().write_atomic(&blobs.join(&digest), data).map_err(errf)?;
        Ok(digest)
    };

    let layer_digest = sha256::digest(&layer);
    match fs().hard_link(result, &blobs.join(&layer_digest)) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(_) => {
            fs().write_atomic_bytes(&blobs.join(&layer_digest), &layer, TAR_MODE).map_err(errf)?
        }
    }

    let config = format!(
        r#"{{"architecture":"{arch}","os":"linux","config":{{}},"rootfs":{{"type":"layers","diff_ids":["sha256:{diff_id}"]}}}}"#,
        arch = oci_arch(),
        diff_id = sha256::digest(&tar),
    );
    let manifest = format!(
        r#"{{"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{{"mediaType":"application/vnd.oci.image.config.v1+json","digest":"sha256:{config_digest}","size":{config_size}}},"layers":[{{"mediaType":"application/vnd.oci.image.layer.v1.tar+zstd","digest":"sha256:{layer_digest}","size":{layer_size}}}]}}"#,
        config_digest = blob(&config)?,
        config_size = config.len(),
        layer_size = layer.len(),
    );
    let digest = blob(&manifest)?;
    let index = format!(
        r#"{{"schemaVersion":2,"manifests":[{{"mediaType":"application/vnd.oci.image.manifest.v1+json","digest":"sha256:{digest}","size":{size}}}]}}"#,
        size = manifest.len(),
    );

    for (name, data) in [
        ("oci-layout", r#"{"imageLayoutVersion":"1.0.0"}"#),
        ("index.json", &index),
        (MANIFEST_DIGEST, &digest),
    ] {
        fs().write_atomic(&layout.join(name), data).map_err(errf)?;
    }
    Ok(digest)
}

// NOTE: builds are given `--platform=local`: buildx takes that to mean the client's platform,
// not the builder's. These images hold no code that runs as part of being a base image anyway.
fn oci_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        arch => arch,
    }
}

#[test]
fn results_become_contexts_once_stored() {
    use ruzstd::encoding::{CompressionLevel, compress_to_vec};

    use crate::sys::{Sys, fake::FakeFs};

    let fakefs = FakeFs::new();
    let _guard = Sys::install(Sys { fs: fakefs.clone(), ..Sys::fake() });
    let (dirs, stage) = (Dirs::faked(), Stage::new("out-3333333333333333").unwrap());
    assert_eq!(dirs.result_as_context(&stage).unwrap(), None);

    let zst = compress_to_vec(&b"a tarball"[..], CompressionLevel::Fastest);
    let result = dirs.result_from_stage(&stage);
    fs().create_dir_all(result.parent().unwrap()).unwrap();
    fs().write_atomic_bytes(&result, &zst, TAR_MODE).unwrap();

    let context = dirs.result_as_context(&stage).unwrap().unwrap();
    let layout = "/results/oci/33/out-3333333333333333";
    assert!(context.uri.as_str().starts_with(&format!("oci-layout://{layout}@sha256:")));
    let layer = Utf8Path::new(layout).join("blobs/sha256").join(sha256::digest(&zst));
    assert!(fs().exists(&layer));
    assert_eq!(dirs.result_as_context(&stage).unwrap(), Some(context), "idempotent");
}
