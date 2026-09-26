use std::{
    fs::{self, OpenOptions},
    io::Write,
};

use anyhow::{Result, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexSet;
use log::info;
use serde::{Deserialize, Serialize};

use crate::{
    green::Green,
    md::{BuildContext, DIESES, Md},
};

#[derive(Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct Final {
    #[doc = envdocs!(CARGOGREEN_FINAL_PATH)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "final-path")]
    pub(crate) path: Option<Utf8PathBuf>,
}

#[must_use]
fn strip_comments(doc: &str) -> String {
    let mut buf = String::new();
    for line in doc.lines() {
        if !line.starts_with(DIESES) {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    buf
}

#[must_use]
fn render_reproducer(contexts: &IndexSet<BuildContext>, call: &str, envs: &str) -> String {
    let mut buf = String::new();
    buf.push('\n');
    buf.push_str("# Pipe this file to");
    if !contexts.is_empty() {
        //TODO: or additional-build-arguments
        buf.push_str(" (not portable due to usage of local build contexts)");
    }
    buf.push_str(&format!(":\n# {envs} \\\n"));
    buf.push_str(&format!("#   {call} <THIS_FILE\n"));
    buf
}

#[must_use]
fn render_trailing_stage(md: Option<&str>, final_stage: &str) -> String {
    let mut buf = String::new();
    if let Some(md) = md {
        buf.push('\n');
        for line in md.lines() {
            Md::comment_pretty(line, &mut buf);
        }
    }
    buf.push('\n');
    buf.push_str(final_stage);
    buf
}

impl Green {
    #[must_use]
    pub(crate) fn is_primary(&self) -> bool {
        self.env(CARGO_PRIMARY_PACKAGE!()).map(|x| x == "1").unwrap_or_default()
    }

    // NOTE: using $CARGO_PRIMARY_PACKAGE still makes >1 hits in rustc calls history: lib + bin, at least.
    #[must_use]
    fn should_write_final_path(&self) -> Option<&Utf8Path> {
        if let Some(path) = self.r#final.path.as_deref()
            && (self.finalpathnonprimary() || self.is_primary())
        {
            return Some(path);
        }
        None
    }

    pub(crate) fn maybe_write_final_path(
        &self,
        containerfile: &Utf8Path,
        contexts: &IndexSet<BuildContext>,
        call: &str,
        envs: &str,
    ) -> Result<()> {
        let Some(path) = self.should_write_final_path() else { return Ok(()) };

        info!("reading (RO) containerfile {containerfile}");
        let mut opts = OpenOptions::new();
        let mut fbuf = if self.finalpathcomments() {
            let _ = fs::copy(containerfile, path)?;
            info!("writing (AW) final path {path}");
            opts.append(true);
            String::new()
        } else {
            let whole = fs::read_to_string(containerfile)
                .map_err(|e| anyhow!("Failed opening (RO) {containerfile}: {e}"))?;
            info!("writing (TW) final path {path}");
            opts.create(true).write(true).truncate(true);
            strip_comments(&whole)
        };

        let call = call.replace(self.paths.cwd.as_str(), "$PWD");
        fbuf.push_str(&render_reproducer(contexts, &call, envs));

        let mut file = opts.open(path)?;
        write!(file, "{fbuf}")?;
        Ok(())
    }

    pub(crate) fn maybe_append_to_final_path(
        &self,
        md_path: &Utf8Path,
        final_stage: String,
    ) -> Result<()> {
        let Some(path) = self.should_write_final_path() else { return Ok(()) };
        info!("appending (AW) to final path {path}");

        let md = self
            .finalpathcomments()
            .then(|| {
                fs::read_to_string(md_path)
                    .map_err(|e| anyhow!("Failed opening (RO) {md_path}: {e}"))
            })
            .transpose()?;

        let fbuf = render_trailing_stage(md.as_deref(), &final_stage);

        let mut file = OpenOptions::new().append(true).open(path)?;
        write!(file, "{fbuf}")?;
        Ok(())
    }
}
