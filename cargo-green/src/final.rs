use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
};

use anyhow::Result;
use camino::Utf8Path;
use indexmap::IndexSet;
use log::info;

use crate::{
    green::Green,
    md::{BuildContext, Md},
};

impl Green {
    // NOTE: using $CARGO_PRIMARY_PACKAGE still makes >1 hits in rustc calls history: lib + bin, at least.
    pub(crate) fn maybe_write_final_path(
        &self,
        containerfile: &Utf8Path,
        contexts: &IndexSet<BuildContext>,
        call: &str,
        envs: &str,
    ) -> Result<()> {
        if let Some(path) = self.final_path.as_deref() {
            if self.final_path_nonprimary || env::var("CARGO_PRIMARY_PACKAGE").is_ok() {
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
                fbuf.push_str(&format!("#   {call} <{}\n", containerfile.file_name().unwrap()));

                let mut file = OpenOptions::new().append(true).open(path)?;
                write!(file, "{fbuf}")?;
            }
        }
        Ok(())
    }

    pub(crate) fn maybe_append_to_final_path(&self, md_path: &Utf8Path) -> Result<()> {
        if let Some(path) = self.final_path.as_deref() {
            if self.final_path_nonprimary || env::var("CARGO_PRIMARY_PACKAGE").is_ok() {
                info!("appending (AW) to final path {path}");

                let mut fbuf = String::new();

                fbuf.push('\n');
                for md_line in fs::read_to_string(md_path)?.lines() {
                    Md::comment_pretty(md_line, &mut fbuf);
                }

                let mut file = OpenOptions::new().append(true).open(path)?;
                write!(file, "{fbuf}")?;
            }
        }
        Ok(())
    }
}
