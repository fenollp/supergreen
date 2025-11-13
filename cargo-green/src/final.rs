use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
};

use anyhow::Result;
use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexSet;
use log::info;
use serde::{Deserialize, Serialize};

use crate::{
    green::Green,
    md::{BuildContext, Md},
};

#[macro_export]
macro_rules! ENV_FINAL_PATH {
    () => {
        "CARGOGREEN_FINAL_PATH"
    };
}

#[derive(Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct Final {
    #[doc = include_str!(concat!("../docs/",ENV_FINAL_PATH!(),".md"))]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "final-path")]
    pub(crate) path: Option<Utf8PathBuf>,
}

pub(crate) fn is_primary() -> bool {
    env::var("CARGO_PRIMARY_PACKAGE").is_ok()
}

impl Green {
    // NOTE: using $CARGO_PRIMARY_PACKAGE still makes >1 hits in rustc calls history: lib + bin, at least.
    fn should_write_final_path(&self) -> Option<&Utf8Path> {
        if let Some(path) = self.r#final.path.as_deref() {
            if self.finalpathnonprimary() || is_primary() {
                return Some(path);
            }
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
        if let Some(path) = self.should_write_final_path() {
            info!("writing (RW) final path {path}");

            let _ = fs::copy(containerfile, path)?;

            let mut fbuf = String::new();

            fbuf.push('\n');
            fbuf.push_str("# Pipe this file to");
            if !contexts.is_empty() {
                //TODO: or additional-build-arguments
                fbuf.push_str(" (not portable due to usage of local build contexts)");
            }
            fbuf.push_str(&format!(":\n# {envs} \\\n"));
            fbuf.push_str(&format!("#   {call} <THIS_FILE\n"));

            let mut file = OpenOptions::new().append(true).open(path)?;
            write!(file, "{fbuf}")?;
        }
        Ok(())
    }

    pub(crate) fn maybe_append_to_final_path(
        &self,
        md_path: &Utf8Path,
        final_stage: String,
    ) -> Result<()> {
        if let Some(path) = self.should_write_final_path() {
            info!("appending (AW) to final path {path}");

            let mut fbuf = String::new();

            fbuf.push('\n');
            for md_line in fs::read_to_string(md_path)?.lines() {
                Md::comment_pretty(md_line, &mut fbuf);
            }

            fbuf.push('\n');
            fbuf.push_str(&final_stage);

            let mut file = OpenOptions::new().append(true).open(path)?;
            write!(file, "{fbuf}")?;
        }
        Ok(())
    }
}
