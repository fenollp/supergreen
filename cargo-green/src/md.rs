// Our own MetaData utils

use std::{collections::HashMap, env, fmt, fs, io::ErrorKind, rc::Rc, str::FromStr};

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use indexmap::{IndexMap, IndexSet};
use log::{info, trace, warn};
use serde::{Deserialize, Serialize};
use szyk::Node;

use crate::{
    build::SOURCE_DATE_EPOCH,
    green::Green,
    logging::maybe_log,
    stage::{AsBlock, AsStage, NamedStage, Script, Stage, RST},
    target_dir::virtual_target_dir,
    PKG,
};

pub(crate) const DIESES: &str = "##";

#[derive(Debug, Clone, Deserialize, Serialize, Eq)]
pub(crate) struct NamedMount {
    pub(crate) name: Stage,
    pub(crate) mount: Utf8PathBuf,
}

/// For use by IndexSet
impl PartialEq for NamedMount {
    fn eq(&self, other: &Self) -> bool {
        self.mount == other.mount
    }
}

/// For use by IndexSet
impl std::hash::Hash for NamedMount {
    fn hash<H>(&self, state: &mut H)
    where
        H: std::hash::Hasher,
    {
        self.mount.hash(state);
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Md {
    this: MdId,

    #[serde(default, skip_serializing_if = "IndexSet::is_empty")]
    externs: IndexSet<MountExtern>,

    #[serde(default, skip_serializing_if = "IndexSet::is_empty")]
    deps: IndexSet<MdId>,

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
            this,

            externs: IndexSet::default(),
            deps: IndexSet::default(),
            buildrs: false,
            buildrs_results: IndexSet::default(),
            writes_to: None,
            mounts: IndexSet::default(),
            set_envs: IndexMap::default(),
            contexts: IndexSet::default(),
            stages: IndexSet::default(),
            writes: vec![],
            stdout: vec![],
            stderr: vec![],
        }
    }
}

impl Md {
    pub(crate) fn build_script_writes_to(&mut self, to: Utf8PathBuf) {
        self.buildrs = true;
        self.writes_to = Some(to);
    }

    #[must_use]
    pub(crate) fn this(&self) -> MdId {
        self.this
    }

    pub(crate) fn externs(&self) -> impl Iterator<Item = &MountExtern> {
        self.externs.iter()
    }

    pub(crate) fn deps(&self) -> impl Iterator<Item = MdId> + use<'_> {
        self.deps.iter().cloned()
    }

    fn from_file(path: &Utf8Path) -> Result<Self> {
        info!("opening (RO) md {path}");
        let txt = fs::read_to_string(path).map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                warn!("couldn't find Md, unexpectedly: suggesting a clean slate");
                return anyhow!(
                    r#"
    Looks like `{PKG}` ran on an unkempt project. That's alright!
    Let's remove the current $CARGO_TARGET_DIR {target_dir}
    then run your command again.
"#,
                    target_dir = env::var("CARGO_TARGET_DIR").unwrap_or_default(),
                );
            }

            anyhow!("Failed reading Md {path}: {e}")
        })?;

        Self::from_str(&txt).map_err(|e| anyhow!("Failed deserializing Md {path}: {e}"))
    }

    pub(crate) fn write_to(&self, path: &Utf8Path) -> Result<()> {
        let md_ser =
            self.to_string_pretty().map_err(|e| anyhow!("Failed serializing Md {path}: {e}"))?;

        info!("opening (RW) Md {path}");
        fs::write(path, md_ser).map_err(|e| anyhow!("Failed creating {path}: {e}"))?;

        if maybe_log().is_some() {
            match fs::read_to_string(path) {
                Ok(data) => data,
                Err(e) => e.to_string(),
            }
            .lines()
            .filter(|x| !x.is_empty())
            .for_each(|line| trace!("❯ {line}"));
        }

        Ok(())
    }

    fn to_string_pretty(&self) -> Result<String> {
        if !self.stages.iter().any(NamedStage::is_rust) {
            bail!("Md is missing root stage {RST}")
        }
        toml::to_string_pretty(self).map_err(Into::into)
    }

    #[must_use]
    fn rust_stage(&self) -> String {
        format!(
            "{}\nARG SOURCE_DATE_EPOCH={SOURCE_DATE_EPOCH}\n",
            &self.stages.iter().find(|ns| ns.is_rust()).and_then(AsBlock::as_block).unwrap()
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
        externs: IndexSet<String>,
        out_dir_var: Option<Utf8PathBuf>,
        target_path: &Utf8Path,
    ) -> Result<Vec<Rc<Self>>> {
        let mut mds = Mds::new(target_path);

        let has_rmetas = externs.iter().any(|xtern| xtern.ends_with(".rmeta"));
        let extern_mdids = self.walk_transitives(&mut mds, externs)?;
        let mut extern_mds = self.keep_result_providers(&mut mds, extern_mdids, has_rmetas)?;

        assert_eq!(self.deps().count(), 0);

        if let Some(out_dir) = out_dir_var {
            extern_mds.push(self.mount_buildrs_output(&mut mds, out_dir)?);
        }

        for buildrs_result in &self.buildrs_results {
            let br_md = mds.load(*buildrs_result)?;
            extern_mds.extend(mds.load_all(br_md.deps())?);
            extern_mds.push(br_md);
        }

        let mds = self.sort_deps(extern_mds)?;
        info!("sorted {} deps", mds.len());

        Ok(mds)
    }

    /// Aggregate deps and mounts from transitive deps
    fn walk_transitives(
        &mut self,
        mds: &mut Mds,
        externs: IndexSet<String>,
    ) -> Result<IndexSet<MdId>> {
        let mut extern_mdids = IndexSet::new();

        for xtern in externs {
            // E.g. libproc_macro2-e44df32b5d502568.rmeta
            trace!("❯ extern {xtern}");
            let xtern = MdId::from_extern_filename(&xtern)?;

            extern_mdids.insert(xtern);

            let extern_md = mds.load(xtern)?;
            self.buildrs_results.extend(extern_md.buildrs_results.iter());
            for transitive in &extern_md.deps {
                trace!("❯ transitive {transitive}");
                let trans_md = mds.load(*transitive)?;
                if let Some(ref out_dir) = trans_md.writes_to {
                    let skip = trans_md.writes.is_empty();
                    info!("{}mounting buildrs out dir {out_dir}", if skip { "skip " } else { "" });
                    if !skip {
                        let mount = out_dir.clone();
                        self.mounts.insert(NamedMount { name: trans_md.last_stage(), mount });
                    }
                } else {
                    extern_mdids.insert(*transitive);
                }
            }
        }

        Ok(extern_mdids)
    }

    /// Keep deps that actually provide files to mount
    fn keep_result_providers(
        &mut self,
        mds: &mut Mds,
        extern_mdids: IndexSet<MdId>,
        has_rmetas: bool,
    ) -> Result<Vec<Rc<Self>>> {
        let mut extern_mds: Vec<Rc<Self>> = vec![];

        for dep in extern_mdids {
            let dep_md = mds.load(dep)?;
            let dep_stage = Stage::output(dep)?;
            self.externs.extend(
                dep_md
                    .writes
                    .iter()
                    .filter(|w: &&Utf8PathBuf| !w.as_str().ends_with(".d"))
                    .filter(|w: &&Utf8PathBuf| has_rmetas || !w.as_str().ends_with(".rmeta"))
                    .filter(|_| !dep_md.buildrs) // Never need transitive deps' build scripts
                    .map(|w| w.file_name().unwrap().to_owned())
                    .map(|xtern: String| MountExtern { from: dep_stage.clone(), xtern }),
            );
            extern_mds.push(dep_md);
        }

        Ok(extern_mds)
    }

    /// For build scripts: when $OUT_DIR is set.
    ///
    /// Turn that $OUT_DIR path to an MdId and
    /// * include it as a dep
    /// * include it as a mount
    /// * aggregate the envs it set
    fn mount_buildrs_output(&mut self, mds: &mut Mds, out_dir: Utf8PathBuf) -> Result<Rc<Self>> {
        let z_dep = MdId::from_out_dir_var(&out_dir);

        self.buildrs_results.insert(z_dep);

        let z_dep_md = mds.load(z_dep)?;
        info!("also mounting {z_dep}'s buildrs out dir {out_dir}");
        self.mounts.insert(NamedMount {
            name: z_dep_md.last_stage(),
            mount: virtual_target_dir(&out_dir),
        });

        for (var, val) in &z_dep_md.set_envs {
            self.set_envs.entry(var.to_owned()).or_insert_with(|| val.to_owned());
        }

        Ok(z_dep_md)
    }

    pub(crate) fn sort_deps(&mut self, mds: Vec<Rc<Self>>) -> Result<Vec<Rc<Self>>> {
        let mut dag: Vec<_> = mds
            .into_iter()
            .map(|md| {
                self.deps.insert(md.this);
                self.contexts.extend(md.contexts.iter().cloned());
                Node::new(md.this, md.deps().collect(), Rc::clone(&md))
            })
            .collect();
        dag.push(Node::new(self.this, self.deps().collect(), Rc::new(self.clone())));

        let mut sorted = szyk::sort(&dag, self.this)
            .map_err(|e| anyhow!("Failed toposorting {}: {e:?}", self.this))?;
        let last = sorted.pop();
        assert_eq!(last.map(|md| md.this), Some(self.this));

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

    fn block_along_with_predecessors(&self, mds: &[Rc<Self>]) -> String {
        let mut blocks = String::new();
        let mut visited = IndexSet::new();
        for md in mds {
            md.append_blocks(&mut blocks, &mut visited);
            blocks.push('\n');
            for line in toml::to_string_pretty(md.as_ref()).expect("previously enc").lines() {
                Self::comment_pretty(line, &mut blocks);
            }
            blocks.push('\n');
        }
        self.append_blocks(&mut blocks, &mut visited);
        blocks
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

        let mut containerfile = green.new_containerfile();
        containerfile.pushln(&self.rust_stage());
        containerfile.nl();
        containerfile.push(&self.block_along_with_predecessors(mds));
        containerfile.write_to(&containerfile_path)?;

        Ok((md_path, containerfile_path))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Eq)]
pub(crate) struct MountExtern {
    pub(crate) from: Stage,
    pub(crate) xtern: String,
}

/// For use by IndexSet
impl PartialEq for MountExtern {
    fn eq(&self, other: &Self) -> bool {
        self.xtern == other.xtern
    }
}

/// For use by IndexSet
impl std::hash::Hash for MountExtern {
    fn hash<H>(&self, state: &mut H)
    where
        H: std::hash::Hasher,
    {
        self.xtern.hash(state);
    }
}

/// A file cache
#[derive(Debug)]
pub(crate) struct Mds {
    target_path: Utf8PathBuf,
    cache: HashMap<MdId, Rc<Md>>,
}

impl Mds {
    pub(crate) fn new(path: &Utf8Path) -> Self {
        Self { target_path: path.to_owned(), cache: HashMap::default() }
    }

    pub(crate) fn load(&mut self, mdid: MdId) -> Result<Rc<Md>> {
        if let Some(md) = self.cache.get(&mdid) {
            return Ok(Rc::clone(md));
        }
        let md = Md::from_file(&mdid.path(&self.target_path))?;
        let md = Rc::new(md);
        let _ = self.cache.insert(mdid, Rc::clone(&md));
        Ok(md)
    }

    pub(crate) fn load_all(&mut self, mdids: impl Iterator<Item = MdId>) -> Result<Vec<Rc<Md>>> {
        mdids.map(|mdid| self.load(mdid)).collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct BuildContext {
    pub(crate) name: Stage,
    /// Actually any BuildKit ctx works, we just only use local paths.
    pub(crate) uri: Utf8PathBuf,
}

/// An ID unique to crate+version+crate-type+.. extracted from the rustc arg "extrafn"
#[derive(Debug, Copy, Clone, Deserialize, Serialize, Eq, PartialEq, Hash)]
#[serde(from = "String", into = "String")]
pub(crate) struct MdId(u64);

impl fmt::Display for MdId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:0>16}", format!("{:#x}", self.0).trim_start_matches("0x"))
    }
}

/// Used by serde
impl From<MdId> for String {
    fn from(metadata: MdId) -> Self {
        format!("{metadata}")
    }
}

/// Used by serde
/// = help: the trait `From<std::string::String>` is not implemented for `md::MdId`
///         but trait `From<&str>` is implemented for it
// TODO? prefer &str impl
impl From<String> for MdId {
    fn from(hex: String) -> Self {
        hex.as_str().into()
    }
}
impl From<&str> for MdId {
    fn from(hex: &str) -> Self {
        assert_eq!(hex.len(), 16, "Unexpected MdId {hex:?}");
        Self(u64::from_str_radix(hex, 16).expect("16-digit hex str"))
    }
}

impl MdId {
    #[must_use]
    pub(crate) fn new(extrafn: &str) -> Self {
        assert!(extrafn.starts_with('-'), "Unexpected extrafn {extrafn:?}");
        extrafn[1..].to_owned().into()
    }

    /// E.g. libunicode_xid-c443c88a44e24bc6.rlib
    fn from_extern_filename(xtern: &str) -> Result<Self> {
        let Some(xtern) = xtern.split(['-', '.']).nth(1) else {
            bail!("BUG: expected extern to match ^lib[^.-]+-<mdid>.[^.]+$: {xtern}")
        };
        Ok(xtern.into())
    }

    /// E.g. OUT_DIR="/tmp/clis-vixargs_0-1-0/release/build/proc-macro-error-attr-de2f43c37de3bfce/out"
    #[must_use]
    fn from_out_dir_var(out_dir: &Utf8Path) -> Self {
        assert_eq!(out_dir.file_name(), Some("out"), "BUG: unexpected $OUT_DIR={out_dir} format");
        out_dir
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            //   => "proc-macro-error-attr-de2f43c37de3bfce"
            .rsplit('-')
            .next()
            .unwrap()
            //   => "de2f43c37de3bfce"
            .into()
    }

    #[must_use]
    fn path(&self, target_path: &Utf8Path) -> Utf8PathBuf {
        target_path.join(format!("{self}.toml"))
    }
}

#[test]
fn mdid_path() {
    assert_eq!(
        MdId(0xfb7fae2e3366cafc).path("some/path".into()),
        "some/path/fb7fae2e3366cafc.toml"
    );
}

#[test]
fn mdid_from_out_dir_var() {
    let out_dir_var = "$CARGO_TARGET_DIR/release/build/proc-macro-error-attr-de2f43c37de3bfce/out";
    assert_eq!(MdId::from_out_dir_var(out_dir_var.into()), "de2f43c37de3bfce".into());
}

#[test]
fn mdid_roundrobin() {
    let extrafn = "-dab737da4696ee62";
    let mdid = MdId::new(extrafn);
    assert_eq!(format!("-{mdid}"), extrafn);
}

#[test]
fn mdid_pads() {
    let mdid = MdId(0x572f583993dd3d9).to_string();
    assert_eq!(mdid, "0572f583993dd3d9");
    assert_eq!(mdid.len(), 16);
}

#[test]
fn mdid_ser() {
    let mdid = MdId(0x78d0c09fd98410d3);
    assert_eq!(mdid.to_string(), "78d0c09fd98410d3".to_owned());
    assert_eq!(&serde_json::to_string(&mdid).unwrap(), "\"78d0c09fd98410d3\"");
}

#[test]
fn mdid_de() {
    let hex = "\"78d0c09fd98410d3\"";
    assert_eq!(serde_json::from_str::<MdId>(hex).unwrap(), MdId(0x78d0c09fd98410d3));
}

#[test]
fn md_ser() {
    use crate::stage::RUST;

    let md = Md {
        this: MdId(0x711ba64e1183a234),
        externs: [MountExtern { from: RUST.clone(), xtern: "blop".into() }].into(),
        deps: [MdId(0x81529f4c2380d9ec), MdId(0x88a4324b2aff6db9)].into(),
        buildrs: false,
        buildrs_results: [MdId(0xa2ba26818f759606)].into(),
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
from = "rust-base"
xtern = "blop"

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
    use crate::cratesio::HOME;

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
            uri: format!(
                "/home/maison/.cargo/{HOME}/github.com-1ecc6299db9ec823/rustversion-1.0.9"
            )
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
    assert_eq!(md.this, MdId(0x9494aa6093cd94c9));
    assert_eq!(md.deps().collect::<Vec<_>>(), vec![MdId(0x0dc1fe2644e3176a)]);
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
