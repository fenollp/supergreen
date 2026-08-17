use std::env;

use anyhow::{Result, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexSet;
use log::info;
use serde::{Deserialize, Serialize};

use crate::{
    green::Green,
    md::{BuildContext, DIESES, Md},
    sys::sys,
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

pub(crate) fn is_primary() -> bool {
    env::var(CARGO_PRIMARY_PACKAGE!()).is_ok()
}

/// Drop the `##`-prefixed Md dump that [`Md::comment_pretty`] interleaves into a
/// Containerfile, keeping only the instructions.
#[must_use]
fn strip_comments(containerfile: &str) -> String {
    let mut buf = String::new();
    for line in containerfile.lines() {
        if !line.starts_with(DIESES) {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    buf
}

/// The `# Pipe this file to: …` trailer describing how to rebuild by hand.
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

/// The Md dump (when enabled) plus the `FROM scratch` stage collecting the artifacts.
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
    // NOTE: using $CARGO_PRIMARY_PACKAGE still makes >1 hits in rustc calls history: lib + bin, at least.
    fn should_write_final_path(&self) -> Option<&Utf8Path> {
        if let Some(path) = self.r#final.path.as_deref()
            && (self.finalpathnonprimary() || is_primary())
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
        let fs = sys().fs;

        info!("reading (RO) containerfile {containerfile}");
        if self.finalpathcomments() {
            fs.copy(containerfile, path)?;
            info!("writing (AW) final path {path}");
        } else {
            let whole = fs
                .read_to_string(containerfile)
                .map_err(|e| anyhow!("Failed opening (RO) {containerfile}: {e}"))?;
            info!("writing (TW) final path {path}");
            fs.write(path, &strip_comments(&whole))?;
        }

        let call = call.replace(self.paths.cwd.as_str(), "$PWD");
        fs.append(path, &render_reproducer(contexts, &call, envs))?;
        Ok(())
    }

    pub(crate) fn maybe_append_to_final_path(
        &self,
        md_path: &Utf8Path,
        final_stage: String,
    ) -> Result<()> {
        let Some(path) = self.should_write_final_path() else { return Ok(()) };
        let fs = sys().fs;
        info!("appending (AW) to final path {path}");

        let md = self
            .finalpathcomments()
            .then(|| {
                fs.read_to_string(md_path)
                    .map_err(|e| anyhow!("Failed opening (RO) {md_path}: {e}"))
            })
            .transpose()?;

        fs.append(path, &render_trailing_stage(md.as_deref(), &final_stage))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use snapbox::{assert_data_eq, str};

    use super::{Final, Green};
    use crate::{
        dirs::Paths,
        sys::{Sys, fake::FakeFs, install},
        testing::assert_containerfile_eq,
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
        let fs = Arc::new(FakeFs::default());
        fs.file(CONTAINERFILE, GENERATED);
        fs
    }

    #[test]
    fn the_md_dump_is_dropped_unless_asked_for() {
        let fs = seeded();
        let _guard = install(Sys { fs: Arc::clone(&fs) as _, ..Sys::fake() });

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
        let _guard = install(Sys { fs: Arc::clone(&fs) as _, ..Sys::fake() });

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
        let _guard = install(Sys { fs: Arc::clone(&fs) as _, ..Sys::fake() });

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
        let _guard = install(Sys { fs: Arc::clone(&fs) as _, ..Sys::fake() });

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
        let _guard = install(Sys { fs: Arc::clone(&fs) as _, ..Sys::fake() });

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
        let _guard = install(Sys { fs: Arc::clone(&fs) as _, ..Sys::fake() });

        let green = Green::default();
        green
            .maybe_write_final_path(CONTAINERFILE.into(), &[].into(), "docker build .", "")
            .unwrap();
        green.maybe_append_to_final_path("/target/whatever.toml".into(), String::new()).unwrap();

        assert_eq!(fs.read(FINAL), None);
        assert_eq!(fs.written(), [CONTAINERFILE]);
    }
}
