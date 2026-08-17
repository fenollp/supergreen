// Our own MetaData utils

use std::{io::ErrorKind, rc::Rc, str::FromStr};

use anyhow::{Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use indexmap::{IndexMap, IndexSet};
use log::{info, trace, warn};
use serde::{Deserialize, Serialize};
use szyk::Node;

use crate::{
    PKG,
    all_our_envs::CARGO_TARGET_DIR,
    build::SOURCE_DATE_EPOCH,
    containerfile::Containerfile,
    green::Green,
    logging::maybe_log,
    stage::{AsBlock, AsStage, NamedStage, RST, Script, Stage},
    sys::sys,
};

mod build_context;
mod md_id;
mod mds;
mod named_mount;

pub(crate) use build_context::*;
pub(crate) use md_id::*;
pub(crate) use mds::*;
pub(crate) use named_mount::*;

pub(crate) const DIESES: &str = "##";

pub(crate) const STAMP: u8 = 1; // Compatibility gate

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Md {
    #[serde(default)]
    stamp: u8,

    this: MdId,

    #[serde(default, skip_serializing_if = "IndexSet::is_empty")]
    externs: IndexSet<NamedMount>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    deps: Vec<MdId>,

    ///

    /// Set when executing a build script (after building it)
    #[serde(default, skip_serializing_if = "<&bool as std::ops::Not>::not")]
    pub(crate) buildrs: bool,

    /// Set when executing buildrs (not when building buildrs)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    writes_to: Option<Utf8PathBuf>,

    #[serde(default, skip_serializing_if = "IndexSet::is_empty")]
    buildrs_results: IndexSet<MdId>,

    #[serde(default, skip_serializing_if = "IndexSet::is_empty")]
    pub(crate) mounts: IndexSet<NamedMount>,

    /// Environment variables set via cargo::rustc-env=VAR=VAL
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub(crate) set_envs: IndexMap<String, String>,

    ///

    /// Out-of-build directories that get mounted (eg. crate code under $PWD)
    #[serde(default, skip_serializing_if = "IndexSet::is_empty")]
    pub(crate) contexts: IndexSet<BuildContext>,

    stages: IndexSet<NamedStage>,

    /// Paths of the files that are the result of the build
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) writes: Vec<Utf8PathBuf>,

    /// Lines written to STDOUT
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) stdout: Vec<String>,

    /// Lines written to STDERR
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) stderr: Vec<String>,
}

impl FromStr for Md {
    type Err = toml::de::Error;
    fn from_str(md_raw: &str) -> Result<Self, Self::Err> {
        toml::de::from_str(md_raw)
    }
}

impl From<MdId> for Md {
    fn from(this: MdId) -> Self {
        Self {
            stamp: STAMP,
            this,

            externs: IndexSet::new(),
            deps: vec![],
            buildrs: false,
            buildrs_results: IndexSet::new(),
            writes_to: None,
            mounts: IndexSet::new(),
            set_envs: IndexMap::new(),
            contexts: IndexSet::new(),
            stages: IndexSet::new(),
            writes: vec![],
            stdout: vec![],
            stderr: vec![],
        }
    }
}

impl Md {
    fn from_out_dir_var(mds: &mut Mds, out_dir: &Utf8Path) -> Result<Rc<Self>> {
        mds.load(MdId::from_out_dir_var(out_dir))
    }

    pub(crate) fn build_script_writes_to(&mut self, to: Utf8PathBuf) {
        self.buildrs = true;
        self.writes_to = Some(to);
    }

    #[must_use]
    pub(crate) fn this(&self) -> MdId {
        self.this
    }

    pub(crate) fn externs(&self) -> impl Iterator<Item = &NamedMount> {
        self.externs.iter()
    }

    pub(crate) fn deps(&self) -> impl Iterator<Item = MdId> + use<'_> {
        self.deps.iter().cloned()
    }

    fn from_file(path: &Utf8Path, target_dir: &Utf8Path) -> Result<Self> {
        info!("opening (RO) md {path}");
        let txt = sys().fs.read_to_string(path).map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                warn!("couldn't find Md, unexpectedly: suggesting a clean slate");
                return anyhow!(
                    r#"
    Looks like `{PKG}` ran on an unkempt project. That's alright!
    Let's remove the current {CARGO_TARGET_DIR} {target_dir}
    then run your command again.
"#
                );
            }

            anyhow!("Failed reading Md {path}: {e}")
        })?;

        let md =
            Self::from_str(&txt).map_err(|e| anyhow!("Failed deserializing Md {path}: {e}"))?;

        if md.stamp != STAMP {
            warn!("found incompatible Md, unexpectedly: suggesting a clean slate");
            bail!(
                r#"
    Md {path} was written by an incompatible `{PKG}` (stamp: {stamp}, expected: {STAMP}).
    Let's remove the current $CARGO_TARGET_DIR {target_dir}
    then run your command again.
"#,
                stamp = md.stamp,
            )
        }

        Ok(md)
    }

    pub(crate) fn write_to(&self, path: &Utf8Path) -> Result<String> {
        let md_ser = self
            .to_string_pretty()
            .map_err(|e| anyhow!("Failed serializing Md {}: {e}", self.this))?;

        info!("opening (Watomic) Md {path}");
        sys().fs.write_atomic(path, &md_ser).map_err(|e| anyhow!("Failed writing {path}: {e}"))?;

        if maybe_log().is_some() {
            match sys().fs.read_to_string(path) {
                Ok(data) => data,
                Err(e) => format!("Failed reading {path}: {e}"),
            }
            .lines()
            .filter(|x| !x.is_empty())
            .for_each(|line| trace!("❯ {line}"));
        }

        Ok(md_ser)
    }

    pub(crate) fn to_string_pretty(&self) -> Result<String> {
        if !self.stages.iter().any(NamedStage::is_rust) {
            bail!("Md is missing root stage {RST}")
        }
        toml::to_string_pretty(self).map_err(Into::into)
    }

    fn out_dir_mount(&self, out_dir: &Utf8Path) -> NamedMount {
        NamedMount { name: self.last_stage(), mount: out_dir.to_owned() }
    }

    #[must_use]
    fn rust_stage(&self) -> String {
        format!(
            "{}\nARG SOURCE_DATE_EPOCH={SOURCE_DATE_EPOCH}\n",
            self.stages.iter().find(|ns| ns.is_rust()).and_then(AsBlock::as_block).unwrap()
        )
    }

    #[must_use]
    pub(crate) fn code_stage(&self) -> Option<&NamedStage> {
        self.stages.iter().find(|ns| {
            let name = ns.name();
            name.is_local() || name.is_remote()
        })
    }

    #[must_use]
    fn last_stage(&self) -> Stage {
        self.stages.last().map(AsStage::name).unwrap().clone()
    }

    pub(crate) fn push_stage(&mut self, ns: &NamedStage) {
        self.stages.insert(ns.clone());
    }

    pub(crate) fn push_block(&mut self, name: &Stage, block: &str) {
        let ns = Script { stage: name.clone(), script: block.trim().to_owned() };
        self.stages.insert(NamedStage::Script(ns));
    }

    fn append_blocks(&self, blocks: &mut String, visited: &mut IndexSet<Stage>) {
        let mut stages = self.stages.iter().filter(|ns| !ns.is_rust());

        let ns = stages.find(|ns| ns.as_block().is_some()).unwrap();
        let name = ns.name();
        let script = ns.as_block().unwrap();

        let mut filter = None;
        if name.is_remote() {
            filter = Some(name);
            if visited.insert(name.to_owned()) {
                blocks.push_str(script.trim());
            }
        } else {
            // Otherwise, write it back in
            blocks.push_str(script.trim());
        }
        blocks.push('\n');

        for ns in stages {
            let name = ns.name();
            if Some(name) == filter {
                continue;
            }
            let Some(script) = ns.as_block() else { continue };
            blocks.push_str(script.trim());
            blocks.push('\n');
        }
    }

    // https://github.com/rust-lang/cargo/issues/12059#issuecomment-1537457492
    //   https://github.com/rust-lang/rust/issues/63012 : Tracking issue for -Z binary-dep-depinfo
    pub(crate) fn assemble_build_dependencies(
        &mut self,
        mds: &mut Mds,
        externs: IndexSet<String>,
        out_dir_var: Option<Utf8PathBuf>,
    ) -> Result<Vec<Rc<Self>>> {
        let has_rmetas = externs.iter().any(|xtern| xtern.ends_with(".rmeta"));

        let (buildrs_results, mounts, extern_mdids) = walk_transitives(mds, externs)?;
        self.mounts = mounts;
        self.buildrs_results = buildrs_results;
        let (filtered, mut extern_mds) = keep_result_providers(mds, extern_mdids, has_rmetas)?;
        self.externs = filtered;

        if let Some(out_dir) = out_dir_var {
            let z_dep_md = Self::from_out_dir_var(mds, &out_dir)?;
            self.buildrs_results.insert(z_dep_md.this);
            info!("also mounting buildrs out dir {out_dir}");
            self.mounts.insert(z_dep_md.out_dir_mount(&out_dir));

            for (var, val) in &z_dep_md.set_envs {
                self.set_envs.entry(var.to_owned()).or_insert_with(|| val.to_owned());
            }
        }

        for buildrs_result in &self.buildrs_results {
            let br_md = mds.load(*buildrs_result)?;
            extern_mds.extend(mds.load_all(br_md.deps())?);
            extern_mds.push(br_md);
        }

        let mds = self.sort_deps(extern_mds)?;
        info!("sorted {} deps", self.deps.len());

        Ok(mds)
    }

    pub(crate) fn sort_deps(&mut self, mds: Vec<Rc<Self>>) -> Result<Vec<Rc<Self>>> {
        let mut dag: Vec<_> = mds
            .iter()
            .map(|md| Node::new(md.this, md.deps().collect(), Some(Rc::clone(md))))
            .collect();
        dag.push(Node::new(self.this, mds.iter().map(|md| md.this).collect(), None));

        let mut sorted =
            szyk::sort(&dag, self.this).map_err(|e| anyhow!("Failed toposorting: {e:?}"))?;
        sorted.pop();
        let sorted: Vec<_> = sorted.into_iter().map(|md| md.expect("wrapped")).collect();

        self.deps = sorted.iter().map(|md| md.this).collect();
        self.contexts.extend(sorted.iter().flat_map(|md| md.contexts.iter().cloned()));
        Ok(sorted)
    }

    pub(crate) fn comment_pretty(line: &str, buf: &mut String) {
        const MAX: usize = u16::MAX as usize - (DIESES.len() + 1 + '\n'.len_utf8());
        let max = MAX.min(line.len());
        //> dockerfile line greater than max allowed size of 65535
        let line = &line[..max];
        if line.is_empty() {
            buf.push_str(DIESES);
            buf.push('\n');
            return;
        }
        buf.push_str(DIESES);
        buf.push(' ');
        buf.push_str(line);
        buf.push('\n');
    }

    fn block_along_with_predecessors(&self, mds: &[Rc<Self>], finalpathcomments: bool) -> String {
        let mut blocks = String::new();
        let mut visited = IndexSet::new();
        for md in mds {
            md.append_blocks(&mut blocks, &mut visited);
            blocks.push('\n');
            if finalpathcomments {
                for line in toml::to_string_pretty(md.as_ref()).expect("previously enc").lines() {
                    Self::comment_pretty(line, &mut blocks);
                }
                blocks.push('\n');
            }
        }
        self.append_blocks(&mut blocks, &mut visited);
        blocks
    }

    /// Assemble this crate's Containerfile: its own stages, preceded by its deps'.
    ///
    /// Pure: the whole point of the split with [`Self::finalize`] is that a test can
    /// snapshot the generated Containerfile without a filesystem.
    #[must_use]
    pub(crate) fn render(&self, green: &Green, mds: &[Rc<Self>]) -> Containerfile {
        let mut containerfile = green.new_containerfile();
        containerfile.pushln(&self.rust_stage());
        containerfile.nl();
        containerfile.push(&self.block_along_with_predecessors(mds, green.finalpathcomments()));
        containerfile
    }

    pub(crate) fn finalize(
        &self,
        green: &Green,
        target_path: &Utf8Path,
        pkg_name: &str,
        mds: &[Rc<Self>],
    ) -> Result<(Utf8PathBuf, Utf8PathBuf)> {
        let md_path = self.this.path(target_path);
        let containerfile_path = target_path.join(format!("{pkg_name}-{}.Dockerfile", self.this));

        self.write_to(&md_path)?;
        self.render(green, mds).write_to(&containerfile_path)?;

        Ok((md_path, containerfile_path))
    }
}

/// Aggregate deps and mounts from transitive deps
fn walk_transitives(
    mds: &mut Mds,
    externs: IndexSet<String>,
) -> Result<(IndexSet<MdId>, IndexSet<NamedMount>, IndexSet<MdId>)> {
    let mut buildrs_results = IndexSet::new();
    let mut mounts = IndexSet::new();
    let mut extern_mdids = IndexSet::new();

    for xtern in externs {
        // E.g. libproc_macro2-e44df32b5d502568.rmeta
        trace!("❯ extern {xtern}");
        let xtern = MdId::from_extern_filename(&xtern)?;

        extern_mdids.insert(xtern);

        let extern_md = mds.load(xtern)?;
        buildrs_results.extend(extern_md.buildrs_results.iter());
        for transitive in &extern_md.deps {
            trace!("❯ transitive {transitive}");
            let trans_md = mds.load(*transitive)?;
            if let Some(ref out_dir) = trans_md.writes_to {
                let skip = trans_md.writes.is_empty();
                info!("{}mounting buildrs out dir {out_dir}", if skip { "skip " } else { "" });
                if !skip {
                    mounts.insert(trans_md.out_dir_mount(out_dir));
                }
            } else {
                extern_mdids.insert(*transitive);
            }
        }
    }

    Ok((buildrs_results, mounts, extern_mdids))
}

/// Keep deps that actually provide files to mount
fn keep_result_providers(
    mds: &mut Mds,
    extern_mdids: IndexSet<MdId>,
    has_rmetas: bool,
) -> Result<(IndexSet<NamedMount>, Vec<Rc<Md>>)> {
    let mut externs = IndexSet::new();
    let mut extern_mds = Vec::with_capacity(extern_mdids.len());

    for dep in extern_mdids {
        let dep_md = mds.load(dep)?;
        let dep_stage = Stage::output(dep)?;
        let dep_has_rmeta = dep_md.writes.iter().any(|w| w.as_str().ends_with(".rmeta"));
        externs.extend(
            dep_md
                .writes
                .iter()
                .filter(|w: &&Utf8PathBuf| !w.as_str().ends_with(".d"))
                .filter(|w: &&Utf8PathBuf| {
                    !if has_rmetas {
                        dep_has_rmeta && w.as_str().ends_with(".rlib")
                    } else {
                        w.as_str().ends_with(".rmeta")
                    }
                })
                .filter(|_| !dep_md.buildrs) // Never need transitive deps' build scripts
                .map(|w| w.file_name().unwrap().to_owned())
                .map(|xtern: String| NamedMount { name: dep_stage.clone(), mount: xtern.into() }),
        );
        extern_mds.push(dep_md);
    }

    Ok((externs, extern_mds))
}

#[test]
fn md_ser() {
    use crate::stage::RUST;

    let md = Md {
        stamp: STAMP,
        this: 0x711ba64e1183a234.into(),
        externs: [NamedMount { name: RUST.clone(), mount: "blop".into() }].into(),
        deps: [0x81529f4c2380d9ec.into(), 0x88a4324b2aff6db9.into()].into(),
        buildrs: false,
        buildrs_results: [0xa2ba26818f759606.into()].into(),
        writes_to: None,
        mounts: [].into(),
        set_envs: [].into(),
        contexts: [BuildContext {
            name: "rust".try_into().unwrap(),
            uri: "/some/local/path".into(),
        }]
        .into(),
        stages: [NamedStage::Script(Script {
            stage: RUST.clone(),
            script: format!("FROM rust AS {RST}"),
        })]
        .into(),
        writes: vec![
            "deps/primeorder-06397107ab8300fa.d".into(),
            "deps/libprimeorder-06397107ab8300fa.rmeta".into(),
            "deps/libprimeorder-06397107ab8300fa.rlib".into(),
        ],
        stdout: vec![],
        stderr: vec![],
    };

    pretty_assertions::assert_eq!(
        r#"
stamp = 1
this = "711ba64e1183a234"
deps = [
    "81529f4c2380d9ec",
    "88a4324b2aff6db9",
]
buildrs_results = ["a2ba26818f759606"]
writes = [
    "deps/primeorder-06397107ab8300fa.d",
    "deps/libprimeorder-06397107ab8300fa.rmeta",
    "deps/libprimeorder-06397107ab8300fa.rlib",
]

[[externs]]
name = "rust-base"
mount = "blop"

[[contexts]]
name = "rust"
uri = "/some/local/path"

[[stages]]

[stages.Script]
stage = "rust-base"
script = "FROM rust AS rust-base"
"#[1..],
        md.to_string_pretty().unwrap()
    );
}

#[test]
fn md_utils() {
    let origin = &r#"
this = "9494aa6093cd94c9"
deps = ["0dc1fe2644e3176a"]
contexts = [
  { name = "input_src_lib_rs--rustversion-1.0.9", uri = "/home/maison/.cargo/registry/src/github.com-1ecc6299db9ec823/rustversion-1.0.9" },
  { name = "crate_out-...", uri = "/home/maison/code/thing.git/target/debug/build/rustversion-ae69baa7face5565/out" },
  { name = "cwd-5b79a479b19b5f41", uri = "/tmp/cwd-5b79a479b19b5f41" },
]
stages = []
"#[1..];

    let contexts = [
        BuildContext {
            name: "input_src_lib_rs--rustversion-1.0.9".try_into().unwrap(),
            uri: "/home/maison/.cargo/registry/src/github.com-1ecc6299db9ec823/rustversion-1.0.9"
                .into(),
        },
        BuildContext {
            name: "crate_out-...".try_into().unwrap(),
            uri: "/home/maison/code/thing.git/target/debug/build/rustversion-ae69baa7face5565/out"
                .into(),
        },
        BuildContext {
            name: "cwd-5b79a479b19b5f41".try_into().unwrap(),
            uri: "/tmp/cwd-5b79a479b19b5f41".into(),
        },
    ];
    let md = Md::from_str(origin).unwrap();
    assert_eq!(md.this, 0x9494aa6093cd94c9.into());
    assert_eq!(md.deps().collect::<Vec<_>>(), vec![0x0dc1fe2644e3176a.into()]);
    dbg!(&md.contexts);
    pretty_assertions::assert_eq!(md.contexts, contexts.clone().into());
}

#[test]
fn md_parsing_failure() {
    let origin = r#"this = "81529f4c2380d9ec"
deps = [[]]
contexts = [
  { name = "rust", uri = "docker-image://docker.io/library/rust:1.77.2-slim@sha256:090d8d4e37850b349b59912647cc7a35c6a64dba8168f6998562f02483fa37d7" },
]
"#;

    let err = Md::from_str(origin).err().map(|x| x.to_string()).unwrap_or_default();
    dbg!(&err);
    assert!(err.contains("\n2 | deps = [[]]\n"));
    assert!(err.contains("\ninvalid type: sequence, expected a string\n"));
}

/// Snapshots of the Containerfile [`Md::render`] assembles for a crate and its deps.
///
/// These stand in for the `recipes/` corpus: same thing being checked — the stage graph
/// that comes out the other end — but on a fixture small enough to read in one screen.
#[cfg(test)]
mod render {
    use std::sync::Arc;

    use camino::Utf8Path;
    use snapbox::str;

    use super::{Md, MdId, Rc};
    use crate::{
        cratesio,
        dirs::Paths,
        green::Green,
        relative,
        stage::{RUST, Stage},
        sys::{Sys, fake::FakeFs, install},
        testing::assert_containerfile_eq,
    };

    /// libc's `lib` compilation, its build script's, and the crate being built.
    fn libc_lib() -> MdId {
        0x1111111111111111_u64.into()
    }
    fn libc_buildrs() -> MdId {
        0x2222222222222222_u64.into()
    }
    fn root() -> MdId {
        0x3333333333333333_u64.into()
    }

    const CARGO_HOME: &str = "/home/u/.cargo";
    const SRC: &str = "/home/u/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f";
    const RUST_BLOCK: &str = "FROM docker.io/library/rust:1.99.0-slim AS rust-base";

    fn paths() -> Paths {
        Paths {
            cargo_home: CARGO_HOME.into(),
            cwd: "/work".into(),
            host_target_dir: Some("/work/target".into()),
            ..Default::default()
        }
    }

    fn green() -> Green {
        Green { paths: paths(), ..Default::default() }
    }

    /// A crates.io dep: its tarball is `ADD`ed from a `FROM scratch` stage.
    async fn cratesio_md(this: MdId, crate_id: &str) -> Md {
        let mut md: Md = this.into();
        md.push_block(&RUST, RUST_BLOCK);
        md.push_stage(
            &cratesio::named_stage(
                &paths(),
                "libc",
                Utf8Path::new(SRC).join("libc-0.2.177").as_path(),
            )
            .await
            .unwrap(),
        );
        let rustc_stage = Stage::dep(crate_id).unwrap();
        md.push_block(
            &rustc_stage,
            &format!(
                "\
FROM rust-base AS {rustc_stage}
WORKDIR /target/debug/deps
RUN \\
  --mount=from=cratesio-libc-0.2.177,source=/libc-0.2.177,dst=$CARGO_HOME/registry/src/index.crates.io/libc-0.2.177 \\
    rustc --crate-name libc --edition=2021 $CARGO_HOME/registry/src/index.crates.io/libc-0.2.177/src/lib.rs"
            ),
        );
        md.out_block(
            &Stage::output(this).unwrap(),
            &rustc_stage,
            &paths(),
            "/work/target/debug/deps".into(),
        );
        md
    }

    /// The crate being built: its source is a local build context, not a stage.
    async fn root_md() -> Md {
        let mut md: Md = root().into();
        md.push_block(&RUST, RUST_BLOCK);
        md.push_stage(&relative::as_stage(root(), "/work".into()).await.unwrap());
        let rustc_stage = Stage::dep("N-mycrate-0.1.0").unwrap();
        md.push_block(
            &rustc_stage,
            &format!(
                "\
FROM rust-base AS {rustc_stage}
WORKDIR /target/debug/deps
WORKDIR /work
RUN \\
  --mount=from=cwd-3333333333333333,source=/src,dst=/work/src \\
  --mount=from=out-1111111111111111,dst=/target/debug/deps/liblibc-1111111111111111.rmeta,source=/deps/liblibc-1111111111111111.rmeta \\
    rustc --crate-name mycrate --edition=2024 src/lib.rs"
            ),
        );
        md.out_block(
            &Stage::output(root()).unwrap(),
            &rustc_stage,
            &paths(),
            "/work/target/debug/deps".into(),
        );
        md
    }

    /// Seeds a workspace and libc's tarball, then renders root + its deps.
    fn render(deps: &[(MdId, &str)]) -> String {
        let fs = Arc::new(FakeFs::default());
        fs.file("/work/Cargo.toml", "[package]\nname = \"mycrate\"\n");
        fs.file("/work/Cargo.lock", "version = 4\n");
        fs.file("/work/src/lib.rs", "pub fn f() {}\n");
        fs.mkdir("/work/.git");
        // Excluded from the build context: cargo's own cache dir, per CACHEDIR.TAG.
        fs.file("/work/target/CACHEDIR.TAG", "Signature: 8a477f597d28d172\n");
        fs.file(
            "/home/u/.cargo/registry/cache/index.crates.io-1949cf8c6b5b557f/libc-0.2.177.crate",
            "<libc tarball>",
        );
        let _guard = install(Sys { fs: Arc::clone(&fs) as _, ..Sys::fake() });

        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
            let mut mds = Vec::new();
            for (dep, crate_id) in deps {
                mds.push(Rc::new(cratesio_md(*dep, crate_id).await));
            }
            root_md().await.render(&green(), &mds).as_str().to_owned()
        })
    }

    #[test]
    fn a_crate_and_its_one_dep() {
        assert_containerfile_eq!(
            render(&[(libc_lib(), "N-libc-0.2.177")]),
            str![[r#"
# syntax=docker.io/docker/dockerfile:1
# check=error=true
# Generated by https://github.com/fenollp/supergreen v0.27.0

FROM docker.io/library/rust:1.99.0-slim AS rust-base
ARG SOURCE_DATE_EPOCH=42


FROM scratch AS cratesio-libc-0.2.177
ADD --chmod=0664 --unpack --checksum=sha256:3e5cb0d37315892d29e2dfd865eaf08bb00b31efd7b7c85b17fcc552d22b5761 \
  https://static.crates.io/crates/libc/libc-0.2.177.crate /
FROM rust-base AS dep-n-libc-0.2.177
WORKDIR /target/debug/deps
RUN \
  --mount=from=cratesio-libc-0.2.177,source=/libc-0.2.177,dst=$CARGO_HOME/registry/src/index.crates.io/libc-0.2.177 \
    rustc --crate-name libc --edition=2021 $CARGO_HOME/registry/src/index.crates.io/libc-0.2.177/src/lib.rs
FROM scratch AS out-1111111111111111
COPY --link --from=dep-n-libc-0.2.177 /target/debug/deps /deps
COPY --link --from=dep-n-libc-0.2.177 /target/debug/out-1111111111111111-* /

FROM rust-base AS dep-n-mycrate-0.1.0
WORKDIR /target/debug/deps
WORKDIR /work
RUN \
  --mount=from=cwd-3333333333333333,source=/src,dst=/work/src \
  --mount=from=out-1111111111111111,dst=/target/debug/deps/liblibc-1111111111111111.rmeta,source=/deps/liblibc-1111111111111111.rmeta \
    rustc --crate-name mycrate --edition=2024 src/lib.rs
FROM scratch AS out-3333333333333333
COPY --link --from=dep-n-mycrate-0.1.0 /target/debug/deps /deps
COPY --link --from=dep-n-mycrate-0.1.0 /target/debug/out-3333333333333333-* /

"#]]
        );
    }

    /// A dep that also runs a build script contributes two Mds sharing one tarball
    /// stage. The `ADD` must be emitted once, or BuildKit sees a duplicate stage name.
    /// [`Md::finalize`] is [`Md::render`] plus two writes; check they land where the
    /// build and the next crate's `Mds::load` will look for them.
    #[test]
    fn finalize_writes_the_md_beside_the_containerfile() {
        let fs = Arc::new(FakeFs::default());
        fs.file("/work/Cargo.toml", "[package]\nname = \"mycrate\"\n");
        fs.file("/work/src/lib.rs", "pub fn f() {}\n");
        let _guard = install(Sys { fs: Arc::clone(&fs) as _, ..Sys::fake() });

        let (md_path, containerfile_path) = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                root_md().await.finalize(&green(), "/work/target/debug".into(), "mycrate", &[])
            })
            .unwrap();

        assert_eq!(md_path, "/work/target/debug/3333333333333333.toml");
        assert_eq!(containerfile_path, "/work/target/debug/mycrate-3333333333333333.Dockerfile");
        assert_eq!(fs.written().len(), 4);

        // The Md is what the next crate reads to learn this one's stages and outputs.
        assert_containerfile_eq!(
            fs.read(&md_path).unwrap(),
            str![[r#"
stamp = 1
this = "3333333333333333"

[[stages]]

[stages.Script]
stage = "rust-base"
script = "FROM docker.io/library/rust:1.99.0-slim AS rust-base"

[[stages]]

[stages.Relative]
stage = "cwd-3333333333333333"
pwd = "/work"
keep = [
    "Cargo.toml",
    "src",
]
lose = []

[[stages]]

[stages.Script]
stage = "dep-n-mycrate-0.1.0"
script = '''
FROM rust-base AS dep-n-mycrate-0.1.0
WORKDIR /target/debug/deps
WORKDIR /work
RUN \
  --mount=from=cwd-3333333333333333,source=/src,dst=/work/src \
  --mount=from=out-1111111111111111,dst=/target/debug/deps/liblibc-1111111111111111.rmeta,source=/deps/liblibc-1111111111111111.rmeta \
    rustc --crate-name mycrate --edition=2024 src/lib.rs'''

[[stages]]

[stages.Script]
stage = "out-3333333333333333"
script = """
FROM scratch AS out-3333333333333333
COPY --link --from=dep-n-mycrate-0.1.0 /target/debug/deps /deps
COPY --link --from=dep-n-mycrate-0.1.0 /target/debug/out-3333333333333333-* /"""

"#]]
        );
    }

    #[test]
    fn a_shared_tarball_stage_is_emitted_once() {
        let rendered =
            render(&[(libc_buildrs(), "X-libc-0.2.177"), (libc_lib(), "N-libc-0.2.177")]);
        assert_eq!(rendered.matches("FROM scratch AS cratesio-libc-0.2.177").count(), 1);
        assert_eq!(rendered.matches("https://static.crates.io/crates/libc/").count(), 1);
        // Both crate stages are still there, each with its own output stage.
        assert_eq!(rendered.matches("FROM rust-base AS dep-x-libc-0.2.177").count(), 1);
        assert_eq!(rendered.matches("FROM rust-base AS dep-n-libc-0.2.177").count(), 1);
        assert_eq!(rendered.matches("FROM scratch AS out-").count(), 3);
    }
}
