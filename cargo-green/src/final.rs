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
    sys::fs,
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
        if let Some(path) = self.should_write_final_path() {
            let mut fbuf = String::new();

            info!("reading (RO) containerfile {containerfile}");
            let mut opts = OpenOptions::new();
            if self.finalpathcomments() {
                let _ = fs::copy(containerfile, path)?;

                info!("writing (AW) final path {path}");
                opts.append(true);
            } else {
                let whole = fs::read_to_string(containerfile)
                    .map_err(|e| anyhow!("Failed opening (RO) {containerfile}: {e}"))?;
                for line in whole.lines() {
                    if !line.starts_with(DIESES) {
                        fbuf.push_str(line);
                        fbuf.push('\n');
                    }
                }

                info!("writing (TW) final path {path}");
                opts.create(true).write(true).truncate(true);
            }

            fbuf.push('\n');
            fbuf.push_str("# Pipe this file to");
            if !contexts.is_empty() {
                //TODO: or additional-build-arguments
                fbuf.push_str(" (not portable due to usage of local build contexts)");
            }
            fbuf.push_str(&format!(":\n# {envs} \\\n"));
            let call = call.replace(self.paths.cwd.as_str(), "$PWD");
            fbuf.push_str(&format!("#   {call} <THIS_FILE\n"));

            let mut file = opts.open(path)?;
            write!(file, "{fbuf}")?;
        }
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
                fs().read_to_string(md_path)
                    .map_err(|e| anyhow!("Failed opening (RO) {md_path}: {e}"))
            })
            .transpose()?;

        fs().append(path, &render_trailing_stage(md.as_deref(), &final_stage))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use snapbox::{assert_data_eq, str};

    use super::{Final, Green};
    use crate::{
        containerfile::assert_containerfile_eq,
        dirs::Paths,
        sys::{Sys, fake::FakeFs},
    };

    const CONTAINERFILE: &str = "/target/crate-0123456789abcdef.Dockerfile";
    const FINAL: &str = "/work/recipe.Dockerfile";

    /// A crate's Containerfile as [`crate::md::Md::finalize`] leaves it: instructions
    /// interleaved with the `##`-prefixed Md dump.
    const GENERATED: &str = "\
FROM rust AS rust-base
##
## this = \"0123456789abcdef\"
##
FROM rust-base AS dep-N-crate
RUN rustc --crate-name crate src/lib.rs
";

    fn green(experiments: &[&str]) -> Green {
        Green {
            r#final: Final { path: Some(FINAL.into()) },
            // Keeps the tests off $CARGO_PRIMARY_PACKAGE.
            experiment: ["finalpathnonprimary"]
                .into_iter()
                .chain(experiments.iter().copied())
                .map(ToOwned::to_owned)
                .collect(),
            paths: Paths { cwd: "/work".into(), ..Default::default() },
            ..Default::default()
        }
    }

    fn seeded() -> Arc<FakeFs> {
        let fs = FakeFs::new();
        fs.file(CONTAINERFILE, GENERATED);
        fs
    }

    #[test]
    fn the_md_dump_is_dropped_unless_asked_for() {
        let fs = seeded();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });

        green(&[])
            .maybe_write_final_path(CONTAINERFILE.into(), &[].into(), "docker build .", "FOO=1")
            .unwrap();

        assert_containerfile_eq!(
            fs.read(FINAL).unwrap(),
            str![[r#"
FROM rust AS rust-base
FROM rust-base AS dep-N-crate
RUN rustc --crate-name crate src/lib.rs

# Pipe this file to:
# FOO=1 \
#   docker build . <THIS_FILE

"#]]
        );
    }

    #[test]
    fn finalpathcomments_keeps_the_md_dump() {
        let fs = seeded();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });

        green(&["finalpathcomments"])
            .maybe_write_final_path(CONTAINERFILE.into(), &[].into(), "docker build .", "FOO=1")
            .unwrap();

        assert_containerfile_eq!(
            fs.read(FINAL).unwrap(),
            str![[r#"
FROM rust AS rust-base
##
## this = "0123456789abcdef"
##
FROM rust-base AS dep-N-crate
RUN rustc --crate-name crate src/lib.rs

# Pipe this file to:
# FOO=1 \
#   docker build . <THIS_FILE

"#]]
        );
    }

    /// A recipe that mounts host directories can't be rebuilt by piping it alone.
    #[test]
    fn local_build_contexts_make_the_recipe_unportable() {
        use crate::md::BuildContext;

        let fs = seeded();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });

        let contexts =
            [BuildContext { name: "crate-src".try_into().unwrap(), uri: "/work".into() }].into();
        green(&[])
            .maybe_write_final_path(CONTAINERFILE.into(), &contexts, "docker build .", "")
            .unwrap();

        assert_data_eq!(
            fs.read(FINAL).unwrap().lines().nth(4).unwrap(),
            str!["# Pipe this file to (not portable due to usage of local build contexts):"]
        );
    }

    /// The recipe is meant to be readable and host-independent.
    #[test]
    fn the_host_cwd_is_hidden_behind_pwd() {
        let fs = seeded();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });

        green(&[])
            .maybe_write_final_path(
                CONTAINERFILE.into(),
                &[].into(),
                "docker build --build-context=src=/work/src /work",
                "",
            )
            .unwrap();

        assert_data_eq!(
            fs.read(FINAL).unwrap().lines().last().unwrap(),
            str!["#   docker build --build-context=src=$PWD/src $PWD <THIS_FILE"]
        );
    }

    #[test]
    fn the_final_stage_is_appended() {
        let fs = seeded();
        fs.file("/target/0123456789abcdef.toml", "this = \"0123456789abcdef\"\n");
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });

        let green = green(&[]);
        green
            .maybe_write_final_path(CONTAINERFILE.into(), &[].into(), "docker build .", "")
            .unwrap();
        green
            .maybe_append_to_final_path(
                "/target/0123456789abcdef.toml".into(),
                "FROM scratch\nCOPY --link --from=out-0123456789abcdef /out/crate /crate\n"
                    .to_owned(),
            )
            .unwrap();

        assert_containerfile_eq!(
            fs.read(FINAL).unwrap(),
            str![[r#"
FROM rust AS rust-base
FROM rust-base AS dep-N-crate
RUN rustc --crate-name crate src/lib.rs

# Pipe this file to:
#  \
#   docker build . <THIS_FILE

FROM scratch
COPY --link --from=out-0123456789abcdef /out/crate /crate

"#]]
        );
    }

    #[test]
    fn without_a_final_path_nothing_is_written() {
        let fs = seeded();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });

        let green = Green::default();
        green
            .maybe_write_final_path(CONTAINERFILE.into(), &[].into(), "docker build .", "")
            .unwrap();
        green.maybe_append_to_final_path("/target/whatever.toml".into(), String::new()).unwrap();

        assert_eq!(fs.read(FINAL), None);
        assert_eq!(fs.written(), [CONTAINERFILE]);
    }
}
