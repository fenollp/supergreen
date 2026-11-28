//! Results as `oci-layout://` build contexts: a result tarball is a valid image layer as is,
//! so an `out-<mdid>` stage's recipe can be swapped for an image made of that one (symlinked) layer.
//!
//! A result's layout is written as soon as the result is: the image's config needs the hash of
//! the uncompressed tarball, which is at hand while compressing it.
//! A result without a layout isn't given as a context: it's never read back whole to make one.
//!
//! # Why result tarballs are enough
//!
//! A dependent only refers to a dependency by the name of its output stage:
//! ```text
//! --mount=from=out-a39581e46fd24859,…,source=/deps/libasync_task-a39581e46fd24859.rmeta
//! ```
//! In the full Containerfile, `out-a395…` copies from `dep-n-async-task-…`, which runs `rustc` on
//! the downloaded source. A build context with the same name replaces `out-a395…` with a ready-made
//! image that has no inputs. BuildKit only runs what the target needs, so the download and `rustc`
//! steps are no longer reachable.
//!
//! This works because `rustc` needs a dependency's compiled files, not its source.
//! Where a step does mount a source, compaction keeps that stage.
//!
//! # Remote builders
//!
//! Should work unchanged. A `docker-container` builder can't see this host's filesystem: `buildx`
//! already sends it the layouts over the build connection. A remote builder would fetch any layer it
//! lacks the same way, and cache it by digest. The cost is the upload.
//! Untried with a remote machine.
//!
//! The same manifests pushed to a registry also work, as `docker-image://…@sha256:…` contexts:
//! the builder then fetches what it lacks by itself.

use std::io::{self, ErrorKind};

use anyhow::{Result, anyhow};
use camino::{Utf8Path, Utf8PathBuf};

use crate::{
    dirs::Dirs,
    ext::sha256_hex,
    md::BuildContext,
    stage::Stage,
    sys::{TAR_MODE, fs},
};

/// What an image says of one of its layers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Layer {
    /// Of the compressed tarball
    pub(crate) digest: String,
    pub(crate) size: u64,
    /// Of the tarball, uncompressed
    pub(crate) diff_id: String,
}

/// Holds the digest of the layout's manifest. Written last: its presence means the layout is whole.
const MANIFEST_DIGEST: &str = "manifest";

impl Dirs {
    /// The build context that stands for `stage`, if its result and layout were stored.
    pub(crate) fn result_as_context(&self, stage: &Stage) -> Result<Option<BuildContext>> {
        if !fs().exists(&self.result_from_stage(stage)) {
            return Ok(None);
        }
        let layout = self.layout(stage);
        let digest = match fs().read_to_string(&layout.join(MANIFEST_DIGEST)) {
            Ok(digest) => digest,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(anyhow!("Failed reading OCI layout {layout}: {e}")),
        };
        let uri = format!("oci-layout://{layout}@sha256:{digest}").into();
        Ok(Some(BuildContext { name: stage.clone(), uri }))
    }

    fn layout(&self, stage: &Stage) -> Utf8PathBuf {
        self.cas.join(stage.bucketed())
    }

    /// (Re)writes the layout of `stage`'s result, which `layer` describes.
    pub(crate) fn write_layout(&self, stage: &Stage, layer: &Layer) -> Result<()> {
        let (result, layout) = (self.result_from_stage(stage), self.layout(stage));
        let errf = |e| anyhow!("Failed writing OCI layout {layout} of {result}: {e}");

        // Not whole until written again, should this fail midway
        match fs().remove_file(&layout.join(MANIFEST_DIGEST)) {
            Err(e) if e.kind() != ErrorKind::NotFound => return Err(errf(e)),
            _ => {}
        }

        let blobs = layout.join("blobs/sha256");
        fs().create_dir_all(&blobs).map_err(errf)?;
        let blob = |data: &str| -> Result<String> {
            let digest = sha256_hex(data);
            fs().write_atomic(&blobs.join(&digest), data).map_err(errf)?;
            Ok(digest)
        };

        // An OCI layout needs the tarball under a second name (its hash): a symlink provides that
        // without a second copy on disk (buildx follows it). Relative, so the cache dir can be moved.
        // Not mandatory: if linking fails we copy, at double the space.
        let link = blobs.join(&layer.digest);
        if fs().symlink_file(&up_to(&link, &result), &link).is_err() {
            let mut copy = fs().atomic_writer(&link, TAR_MODE).map_err(errf)?;
            let _ = io::copy(&mut fs().open(&result).map_err(errf)?, &mut copy).map_err(errf)?;
            copy.commit().map_err(errf)?;
        }

        let config = format!(
            r#"{{"architecture":"{arch}","os":"linux","config":{{}},"rootfs":{{"type":"layers","diff_ids":["sha256:{diff_id}"]}}}}"#,
            arch = oci_arch(),
            diff_id = layer.diff_id,
        );
        let manifest = format!(
            r#"{{"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{{"mediaType":"application/vnd.oci.image.config.v1+json","digest":"sha256:{config_digest}","size":{config_size}}},"layers":[{{"mediaType":"application/vnd.oci.image.layer.v1.tar+zstd","digest":"sha256:{layer_digest}","size":{layer_size}}}]}}"#,
            config_digest = blob(&config)?,
            config_size = config.len(),
            layer_digest = layer.digest,
            layer_size = layer.size,
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
        Ok(())
    }
}

/// `path` as seen from the dir `link` is in
fn up_to(link: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    let dir = link.parent().unwrap_or(link);
    let shared = dir.components().zip(path.components()).take_while(|(a, b)| a == b).count();
    let ups = dir.components().skip(shared).map(|_| "..");
    ups.chain(path.components().skip(shared).map(|c| c.as_str())).collect()
}

// NOTE: builds are given `--platform=local`: buildx resolves that on the client, so this machine's
// architecture matches what the build asks for. Asking the builder would be wrong.
// A deliberately mislabelled `arm64` image was accepted without complaint (amd64 builder).
// `os` is always `linux`: untried from macOS.
fn oci_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        arch => arch,
    }
}

#[test]
fn results_become_contexts_once_stored_with_their_layout() {
    use crate::sys::{Sys, fake::FakeFs};

    let fakefs = FakeFs::new();
    let _guard = Sys::install(Sys { fs: fakefs.clone(), ..Sys::fake() });
    let (dirs, stage) = (Dirs::faked(), Stage::new("out-3333333333333333").unwrap());
    assert_eq!(dirs.result_as_context(&stage).unwrap(), None);

    let result = dirs.result_from_stage(&stage);
    fs().create_dir_all(result.parent().unwrap()).unwrap();
    fs().write_atomic_bytes(&result, b"a tarball", TAR_MODE).unwrap();
    assert_eq!(dirs.result_as_context(&stage).unwrap(), None, "no layout");

    let layer = |digest: &str| Layer { digest: digest.into(), size: 9, diff_id: "d".into() };
    dirs.write_layout(&stage, &layer("abc")).unwrap();
    let context = dirs.result_as_context(&stage).unwrap().unwrap();
    let layout = "/cas/33/out-3333333333333333";
    assert!(context.uri.as_str().starts_with(&format!("oci-layout://{layout}@sha256:")));
    assert_eq!(
        fs().read_link(&Utf8Path::new(layout).join("blobs/sha256/abc")).unwrap(),
        "../../../../../results/33/out-3333333333333333.tar.zst"
    );

    // A result stored anew gets its layout anew
    dirs.write_layout(&stage, &layer("def")).unwrap();
    assert_ne!(dirs.result_as_context(&stage).unwrap(), Some(context));
}
