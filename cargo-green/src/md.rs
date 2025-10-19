// Our own MetaData utils

use std::{collections::HashMap, env, fmt, fs, io::ErrorKind, str::FromStr};

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexSet;
use log::{info, trace, warn};
use serde::{Deserialize, Serialize};
use szyk::{sort, Node, TopsortError};

use crate::{
    logging::maybe_log,
    stage::{Stage, RST, RUST},
    PKG,
};

//FIXME unpub?
#[derive(Debug, Clone, Deserialize, Serialize, Eq)]
pub(crate) struct NamedMount {
    pub(crate) name: Stage,
    pub(crate) src: Utf8PathBuf,
    pub(crate) dst: Utf8PathBuf,
}

/// For use by IndexSet
impl PartialEq for NamedMount {
    fn eq(&self, other: &Self) -> bool {
        self.dst == other.dst
    }
}

/// For use by IndexSet
impl std::hash::Hash for NamedMount {
    fn hash<H>(&self, state: &mut H)
    where
        H: std::hash::Hasher,
    {
        self.dst.hash(state);
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

    #[serde(default, skip_serializing_if = "IndexSet::is_empty")]
    pub(crate) short_externs: IndexSet<MdId>,

    #[serde(default, skip_serializing_if = "IndexSet::is_empty")]
    pub(crate) buildrs_results: IndexSet<MdId>,
    #[serde(default, skip_serializing_if = "IndexSet::is_empty")]
    pub(crate) mounts: IndexSet<NamedMount>,

    #[serde(default, skip_serializing_if = "IndexSet::is_empty")]
    pub(crate) contexts: IndexSet<BuildContext>,

    stages: IndexSet<NamedStage>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) writes: Vec<Utf8PathBuf>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) stdout: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) stderr: Vec<String>,
}

impl FromStr for Md {
    type Err = toml::de::Error;
    fn from_str(md_raw: &str) -> Result<Self, Self::Err> {
        toml::de::from_str(md_raw)
    }
}

impl Md {
    #[must_use]
    pub(crate) fn new(extrafn: &str) -> Self {
        Self {
            this: MdId::new(extrafn),
            externs: IndexSet::default(),
            deps: IndexSet::default(),
            short_externs: IndexSet::default(),
            buildrs_results: IndexSet::default(),
            mounts: IndexSet::default(),
            contexts: IndexSet::default(),
            stages: IndexSet::default(),
            writes: Vec::default(),
            stdout: Vec::default(),
            stderr: Vec::default(),
        }
    }

    #[must_use]
    pub(crate) fn this(&self) -> MdId {
        self.this
    }

    pub(crate) fn externs(&self) -> impl Iterator<Item = &MountExtern> {
        self.externs.iter()
    }

    #[must_use]
    fn deps(&self) -> Vec<MdId> {
        self.deps.iter().cloned().collect()
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
        if !self.stages.iter().any(|NamedStage { name, .. }| *name == *RUST) {
            bail!("Md is missing root stage {RST}")
        }
        toml::to_string_pretty(self).map_err(Into::into)
    }

    #[must_use]
    pub(crate) fn rust_stage(&self) -> &str {
        &self.stages.iter().find(|NamedStage { name, .. }| *name == *RUST).unwrap().script
    }

    #[must_use]
    fn last_stage(&self) -> Stage {
        self.stages.last().unwrap().name.clone()
    }

    pub(crate) fn push_block(&mut self, name: &Stage, block: String) {
        self.stages.insert(NamedStage { name: name.clone(), script: block.trim().to_owned() });
    }

    fn append_blocks(&self, dockerfile: &mut String, visited: &mut IndexSet<Stage>) {
        let mut stages = self.stages.iter().filter(|NamedStage { name, .. }| *name != *RUST);

        let NamedStage { name, script } = stages.next().expect("at least one stage");

        let mut filter = None;
        if name.is_remote() {
            filter = Some(name);
            if visited.insert(name.to_owned()) {
                dockerfile.push_str(script);
            }
        } else {
            // Otherwise, write it back in
            dockerfile.push_str(script);
        }
        dockerfile.push('\n');

        for NamedStage { name, script } in stages {
            if Some(name) == filter {
                continue;
            }
            dockerfile.push_str(script);
            dockerfile.push('\n');
        }
    }

    // https://github.com/rust-lang/cargo/issues/12059#issuecomment-1537457492
    //   https://github.com/rust-lang/rust/issues/63012 : Tracking issue for -Z binary-dep-depinfo
    pub(crate) fn assemble_build_dependencies(
        &mut self,
        externs: IndexSet<String>,
        out_dir_var: Option<Utf8PathBuf>,
        target_path: &Utf8Path,
    ) -> Result<Vec<Self>> {
        let mut mds = HashMap::<Utf8PathBuf, Self>::new(); // A file cache

        let mut extern_mds_and_paths = vec![];

        let no_rmetas = externs.iter().all(|xtern| !xtern.ends_with(".rmeta"));
        let exclude_rmeta_when_not_needed =
            |xtern: &Utf8Path| if no_rmetas { !xtern.as_str().ends_with(".rmeta") } else { true };

        for xtern in externs {
            // E.g. libproc_macro2-e44df32b5d502568.rmeta
            // E.g. libunicode_xid-c443c88a44e24bc6.rlib
            trace!("❯ extern {xtern}");

            // E.g. c443c88a44e24bc6
            let Some(xtern) = xtern.split(['-', '.']).nth(1) else {
                bail!("BUG: expected extern to match ^lib[^.-]+-<mdid>.[^.]+$: {xtern}")
            };
            let xtern = MdId::new(&format!("-{xtern}"));

            trace!("❯ short extern {xtern}");
            self.short_externs.insert(xtern);

            let extern_md = xtern.path(target_path);
            info!("checking (RO) extern's externs {extern_md}");
            let extern_md = get_or_read(&mut mds, &extern_md)?;

            for transitive in extern_md.short_externs {
                trace!("❯ transitive short extern {transitive}");
                self.short_externs.insert(transitive);
            }

            // for buildrs_result in &extern_md.buildrs_results {
            //     let br_md_path = md_pather(buildrs_result);
            //     let br_md = get_or_read(&mut mds, &br_md_path)?;
            //     for dep in &br_md.deps {
            //         let mut dep_md_path = md_pather(&format!("*-{dep}"));
            //         for (i, p) in glob::glob(dep_md_path.as_str()).unwrap().enumerate() {
            //             assert_eq!(i, 0, ">>> {p:?}");
            //             dep_md_path = p.unwrap().try_into().unwrap();
            //         }
            //         let dep_md = get_or_read(&mut mds, &dep_md_path)?;
            //         extern_mds_and_paths.push((dep_md_path, dep_md));
            //     }
            //     extern_mds_and_paths.push((br_md_path, br_md));
            // }
            self.buildrs_results.extend(extern_md.buildrs_results);
            //FIXME? also add transitive buildrs_results?
        }

        for dep in &self.short_externs {
            let dep_md_path = dep.path(target_path);
            let dep_md = get_or_read(&mut mds, &dep_md_path)?;
            let dep_stage = Stage::output(*dep)?;
            self.externs.extend(
                dep_md
                    .writes
                    .iter()
                    .filter(|w: &&Utf8PathBuf| !w.as_str().ends_with(".d"))
                    .filter(|w: &&Utf8PathBuf| exclude_rmeta_when_not_needed(w))
                    .map(|w| w.file_name().unwrap().to_owned())
                    .map(|xtern: String| MountExtern { from: dep_stage.clone(), xtern }),
            );
            extern_mds_and_paths.push((dep_md_path, dep_md));
        }

        if let Some(out_dir) = out_dir_var {
            assert_eq!(
                out_dir.file_name(),
                Some("out"),
                "BUG: unexpected $OUT_DIR={out_dir} format"
            );
            // With OUT_DIR="/tmp/clis-vixargs_0-1-0/release/build/proc-macro-error-attr-de2f43c37de3bfce/out"
            //   => proc-macro-error-attr-de2f43c37de3bfce
            let z_dep = out_dir.parent().unwrap().file_name().unwrap();
            assert_eq!("proc-macro-error-attr-de2f43c37de3bfce", Utf8PathBuf::from("/tmp/clis-vixargs_0-1-0/release/build/proc-macro-error-attr-de2f43c37de3bfce/out").parent().unwrap().file_name().unwrap());
            assert_eq!(
                "de2f43c37de3bfce",
                "proc-macro-error-attr-de2f43c37de3bfce".rsplit('-').next().unwrap()
            );
            let z_dep = z_dep.rsplit('-').next().unwrap();
            let z_dep = MdId::new(&format!("-{z_dep}"));

            // md.short_externs.insert(z_dep.to_owned());
            self.buildrs_results.insert(z_dep);

            let z_dep_md_path = z_dep.path(target_path);
            let z_dep_md = get_or_read(&mut mds, &z_dep_md_path)?;
            let out_dir_mount =
                NamedMount { name: z_dep_md.last_stage(), src: "/".into(), dst: out_dir };
            info!("also mounting buildrs out dir {out_dir_mount:?} from {z_dep_md:?}");
            self.mounts.insert(out_dir_mount);

            // info!("and adding that buildrs dep");
            // // build_script_build-422764cb03f8177b
            // assert_eq!(z_dep_md.deps.len(), 1);
            // let x_dep = format!("build_script_build-{}", z_dep_md.deps[0]);
            // let x_dep_md_path = md_pather(&x_dep);
            // let x_dep_md = get_or_read(&mut mds, &x_dep_md_path)?;
            // info!("and adding that buildrs dep: {x_dep_md:?}");
            // extern_mds_and_paths.push((x_dep_md_path, x_dep_md));

            // md.short_externs.insert(x_dep);

            extern_mds_and_paths.push((z_dep_md_path, z_dep_md));
        }

        for buildrs_result in &self.buildrs_results {
            let br_md_path = buildrs_result.path(target_path);
            let br_md = get_or_read(&mut mds, &br_md_path)?;
            for dep in &br_md.deps {
                let dep_md_path = dep.path(target_path);
                let dep_md = get_or_read(&mut mds, &dep_md_path)?;

                for dep in &dep_md.deps {
                    let dep_md_path = dep.path(target_path);
                    let dep_md = get_or_read(&mut mds, &dep_md_path)?;
                    extern_mds_and_paths.push((dep_md_path, dep_md));
                }

                extern_mds_and_paths.push((dep_md_path, dep_md));
            }
            extern_mds_and_paths.push((br_md_path, br_md));
        }

        let extern_md_paths = self.sort_deps(extern_mds_and_paths)?;
        info!("extern_md_paths: {}", extern_md_paths.len());

        let mds = extern_md_paths
            .into_iter()
            .map(|extern_md_path| get_or_read(&mut mds, &extern_md_path))
            .collect::<Result<Vec<_>>>()?;

        Ok(mds)
    }

    //FIXME: try unpub
    pub(crate) fn sort_deps(&mut self, mds: Vec<(Utf8PathBuf, Self)>) -> Result<Vec<Utf8PathBuf>> {
        let mut dag: Vec<_> = mds
            .into_iter()
            .map(|(md_path, md)| {
                let node = Node::new(md.this, md.deps(), md_path);
                self.deps.insert(md.this);
                self.contexts.extend(md.contexts);
                node
            })
            .collect();
        let this = self.this;
        let root = Utf8PathBuf::new();
        dag.push(Node::new(this, self.deps(), root.clone()));

        let mut md_paths = sort(&dag, this).map_err(|e| match e {
            TopsortError::TargetNotFound(x) => {
                anyhow!("Failed topolosorting {this}: {x} not found")
            }
            TopsortError::CyclicDependency(x) => {
                anyhow!("Failed topolosorting {this}: cyclic {x}")
            }
        })?;
        let last = md_paths.pop();
        assert_eq!(last, Some(root), "BUG: should be self.this's empty path");

        Ok(md_paths)
    }

    pub(crate) fn comment_pretty(line: &str, buf: &mut String) {
        buf.push_str("## ");
        let max = usize::from(u16::MAX) - "## ".len() - '\n'.len_utf8();
        let max = std::cmp::min(max, line.len());
        buf.push_str(&line[..max]);
        buf.push('\n');
        //> dockerfile line greater than max allowed size of 65535
    }

    //FIXME: try unpub via newfn "green.new_containerfile block"
    pub(crate) fn block_along_with_predecessors(&self, mds: &[Self]) -> String {
        let mut blocks = String::new();
        let mut visited_cratesio_stages = IndexSet::new();
        for md in mds {
            md.append_blocks(&mut blocks, &mut visited_cratesio_stages);
            blocks.push('\n');
            for line in toml::to_string_pretty(md).expect("previously enc").lines() {
                Self::comment_pretty(line, &mut blocks);
            }
            blocks.push('\n');
        }
        self.append_blocks(&mut blocks, &mut visited_cratesio_stages);
        blocks
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

//FIXME: unpub via Mds newtype
pub(crate) fn get_or_read(mds: &mut HashMap<Utf8PathBuf, Md>, path: &Utf8Path) -> Result<Md> {
    if let Some(md) = mds.get(path) {
        return Ok(md.clone());
    }
    let md = Md::from_file(path)?;
    let _ = mds.insert(path.to_path_buf(), md.clone());
    Ok(md)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
struct NamedStage {
    name: Stage,
    script: String,
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

/// For use by serde
impl From<MdId> for String {
    fn from(metadata: MdId) -> Self {
        format!("{metadata}")
    }
}

/// For use by serde
// TODO? prefer &str impl
impl From<String> for MdId {
    fn from(hex: String) -> Self {
        assert_eq!(hex.len(), 16, "Unexpected MdId {hex:?}");
        Self(u64::from_str_radix(&hex, 16).expect("16-digit hex str"))
    }
}

impl MdId {
    #[must_use]
    pub(crate) fn new(extrafn: &str) -> Self {
        assert!(extrafn.starts_with('-'), "Unexpected extrafn {extrafn:?}");
        extrafn[1..].to_owned().into()
    }

    #[must_use]
    pub(crate) fn path(&self, target_path: &Utf8Path) -> Utf8PathBuf {
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
    let md = Md {
        this: MdId(0x711ba64e1183a234),
        externs: [MountExtern { from: RUST.clone(), xtern: "blop".into() }].into(),
        deps: [MdId(0x81529f4c2380d9ec), MdId(0x88a4324b2aff6db9)].into(),
        short_externs: [MdId(0xb8c41dbf50ca5479), MdId(0x96a741f581f4126a)].into(),
        buildrs_results: [MdId(0xa2ba26818f759606)].into(),
        mounts: [].into(),
        contexts: [BuildContext {
            name: "rust".try_into().unwrap(),
            uri: "/some/local/path".into(),
        }]
        .into(),
        stages: [NamedStage { name: RUST.clone(), script: format!("FROM rust AS {RST}") }].into(),
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
short_externs = [
    "b8c41dbf50ca5479",
    "96a741f581f4126a",
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
name = "rust-base"
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
  { name = "cwd-5b79a479b19b5f41", uri = "/tmp/CWD5b79a479b19b5f41" },
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
            uri: "/tmp/CWD5b79a479b19b5f41".into(),
        },
    ];
    let md = Md::from_str(origin).unwrap();
    assert_eq!(md.this, MdId(0x9494aa6093cd94c9));
    assert_eq!(md.deps(), vec![MdId(0x0dc1fe2644e3176a)]);
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
