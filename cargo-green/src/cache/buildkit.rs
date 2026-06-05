//! BuildKit's build cache is the runner's artifacts that are required to
//! produce the build results.
//!
//! <https://docs.docker.com/build/cache/backends/local/>

use std::fs;

use anyhow::{anyhow, Result};
use camino::{Utf8Path, Utf8PathBuf};

use crate::{builder::Builder, dirs::Dirs, stage::Stage};

impl Dirs {
    /// Local BuildKit cache export destination for a given stage
    pub(crate) fn new_runner_cache(&self, target: &Stage) -> Result<Option<Utf8PathBuf>> {
        let dst = self.buildkit.join(target.as_str());
        if dst.exists() {
            return Ok(None);
        }
        fs::create_dir_all(&dst).map_err(|e| anyhow!("Failed to `mkdir -p {dst}`: {e}"))?;
        Ok(Some(dst))
    }

    /// Local BuildKit cache import source for a given stage, if any cache was exported there.
    /// The dir may be pre-created yet empty (see [`Self::new_runner_cache`]).
    pub(crate) fn runner_cache(&self, target: &Stage) -> Option<Utf8PathBuf> {
        let src = self.buildkit.join(target.as_str());
        src.join("index.json").is_file().then_some(src)
    }
}

impl Builder {
    pub(crate) fn import_arg(&self, src: &Utf8Path) -> String {
        format!("--cache-from=type=local,src={src}")
    }

    /// NOTE: option "compatibility-version=20" is only about `--output`
    /// * [Add versioning to exporting](https://github.com/moby/buildkit/issues/4629)
    /// * <https://github.com/moby/buildkit/blob/v0.30.0/docs/build-repro.md#compatibility-version>
    pub(crate) fn export_arg(&self, dst: &Utf8Path) -> String {
        let mut arg = "--cache-to=type=local".to_owned();
        arg.push_str(&format!(",dest={dst}"));
        arg.push_str(&format!(",ignore-error={}", "true"));
        //FIXME: decide on exporter options
        arg.push_str(&format!(",mode={}", "max"));
        // arg.push_str(&format!(",mode={}", "min"));
        // arg.push_str(&format!(",oci-mediatypes={}", "false"));
        arg.push_str(&format!(",oci-mediatypes={}", "true"));
        // arg.push_str(&format!(",image-manifest={}", "false"));
        arg.push_str(&format!(",image-manifest={}", "true"));
        arg.push_str(&format!(",compression={}", "gzip"));
        // arg.push_str(&format!(",compression={}", "zstd"));
        // arg.push_str(&format!(",compression={}", "estargz"));
        arg.push_str(&format!(",compression-level={}", "0"));
        // arg.push_str(&format!(",compression-level={}", "22"));
        // arg.push_str(&format!(",compression-level={}", "11"));
        // arg.push_str(&format!(",compression-level={}", "7"));
        // arg.push_str(&format!(",compression-level={}", "3"));
        // arg.push_str(&format!(",compression-level={}", "1"));
        arg.push_str(&format!(",force-compression={}", "false"));
        // arg.push_str(&format!(",force-compression={}", "true"));
        arg
    }
}
