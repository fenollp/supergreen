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
        if self.finalpathcomments() {
            fs().copy(containerfile, path)?;
            info!("writing (AW) final path {path}");
        } else {
            let whole = fs()
                .read_to_string(containerfile)
                .map_err(|e| anyhow!("Failed opening (RO) {containerfile}: {e}"))?;
            info!("writing (TW) final path {path}");
            fs().write(path, &strip_comments(&whole))?;
        }

        let call = call.replace(self.paths.cwd.as_str(), "$PWD");
        fs().append(path, &render_reproducer(contexts, &call, envs))?;
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

    use snapbox::assert_data_eq;

    use super::{Final, Green};
    use crate::{
        dirs::Paths,
        md::BuildContext,
        sys::{Sys, fake::FakeFs},
    };

    const CONTAINERFILE: &str = "/home/u/stuff/crate-0123456789abcdef.Dockerfile";
    const FINAL: &str = "/home/u/dir/recipe.Dockerfile";
    const CWD: &str = "/home/u/wrk";

    /// A crate's Containerfile with [`crate::md::DIESES`]
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
            experiment: ["finalpathnonprimary"]
                .into_iter()
                .chain(experiments.iter().copied())
                .map(ToOwned::to_owned)
                .collect(),
            paths: Paths { cwd: CWD.into(), ..Default::default() },
            ..Default::default()
        }
    }

    fn with(green: Green, f: impl AsyncFnOnce(Green, Arc<FakeFs>) -> ()) {
        let fs = FakeFs::new();
        fs.file(CONTAINERFILE, GENERATED);
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f(green, fs));
    }

    #[test]
    fn without_a_final_path_nothing_is_written() {
        with(Green::default(), async |green, fs| {
            green
                .maybe_write_final_path(CONTAINERFILE.into(), &[].into(), "docker build .", "")
                .unwrap();
            green
                .maybe_append_to_final_path("/home/u/stuff/whatever.toml".into(), "".into())
                .unwrap();

            assert_eq!(fs.read(FINAL), None);
            assert_eq!(fs.written(), [CONTAINERFILE]);
        });
    }

    #[test]
    fn the_md_dump_is_dropped_unless_asked_for() {
        with(green(&[]), async |green, fs| {
            green
                .maybe_write_final_path(CONTAINERFILE.into(), &[].into(), "docker build .", "FOO=1")
                .unwrap();

            assert_snapshots_eq!(
                fs.read(FINAL).unwrap(),
                snapbox::str![[r#"
FROM rust AS rust-base
FROM rust-base AS dep-N-crate
RUN rustc --crate-name crate src/lib.rs

# Pipe this file to:
# FOO=1 \
#   docker build . <THIS_FILE

"#]]
            );
        });
    }

    #[test]
    fn finalpathcomments_keeps_the_md_dump() {
        with(green(&["finalpathcomments"]), async |green, fs| {
            green
                .maybe_write_final_path(CONTAINERFILE.into(), &[].into(), "docker build .", "FOO=1")
                .unwrap();

            assert_snapshots_eq!(
                fs.read(FINAL).unwrap(),
                snapbox::str![[r#"
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
        });
    }

    #[test]
    fn local_build_contexts_make_the_recipe_unportable() {
        with(green(&[]), async |green, fs| {
            let contexts =
                [BuildContext { name: "crate-src".try_into().unwrap(), uri: CWD.into() }];
            green
                .maybe_write_final_path(
                    CONTAINERFILE.into(),
                    &contexts.into(),
                    "docker build .",
                    "",
                )
                .unwrap();

            assert_data_eq!(
                fs.read(FINAL).unwrap().lines().nth(4).unwrap(),
                snapbox::str![[r#"
# Pipe this file to (not portable due to usage of local build contexts):
"#]]
            );
        });
    }

    #[test]
    fn the_host_cwd_is_hidden_behind_pwd() {
        with(green(&[]), async |green, fs| {
            green
                .maybe_write_final_path(
                    CONTAINERFILE.into(),
                    &[].into(),
                    &format!(
                        "docker build --output=. --build-context=cwd-5512fb75cb14751e={CWD} -"
                    ),
                    "",
                )
                .unwrap();

            assert_data_eq!(
                fs.read(FINAL).unwrap().lines().last().unwrap(),
                snapbox::str![[r#"
#   docker build --output=. --build-context=cwd-5512fb75cb14751e=$PWD - <THIS_FILE
"#]]
            );
        });
    }

    #[test]
    fn the_final_stage_is_appended() {
        with(green(&[]), async |green, fs| {
            fs.file("/home/u/stuff/0123456789abcdef.toml", "this = \"0123456789abcdef\"\n");
            green
                .maybe_write_final_path(CONTAINERFILE.into(), &[].into(), "docker build .", "")
                .unwrap();
            green
                .maybe_append_to_final_path(
                    "/home/u/stuff/0123456789abcdef.toml".into(),
                    "FROM scratch\nCOPY --link --from=out-0123456789abcdef /out/crate /crate\n"
                        .into(),
                )
                .unwrap();

            assert_snapshots_eq!(
                fs.read(FINAL).unwrap(),
                snapbox::str![[r#"
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
        });
    }
}
