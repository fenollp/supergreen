// Our own MetaData utils

use std::{env, fs, io::ErrorKind, str::FromStr};

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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Md {
    this: MdId,

    #[serde(default, skip_serializing_if = "IndexSet::is_empty")]
    deps: IndexSet<MdId>,

    #[serde(default, skip_serializing_if = "IndexSet::is_empty")]
    pub(crate) short_externs: IndexSet<String>,

    #[serde(default, skip_serializing_if = "<&bool as std::ops::Not>::not")]
    pub(crate) is_proc_macro: bool,

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
    pub(crate) fn new(this: &str) -> Self {
        Self {
            this: MdId(this.to_owned()),
            deps: [].into(),
            short_externs: [].into(),
            is_proc_macro: false,
            contexts: [].into(),
            stages: [].into(),
            writes: vec![],
            stdout: vec![],
            stderr: vec![],
        }
    }

    pub(crate) fn from_file(path: &Utf8Path) -> Result<Self> {
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

    pub(crate) fn sort_deps(&mut self, mds: Vec<(Utf8PathBuf, Self)>) -> Result<Vec<Utf8PathBuf>> {
        let mut dag: Vec<_> = mds
            .into_iter()
            .map(|(md_path, md)| {
                let this = md.this.as_u64();
                self.deps.insert(md.this);
                self.contexts.extend(md.contexts);
                Node::new(this, md.deps.as_u64s(), md_path)
            })
            .collect();
        let this = self.this.as_u64();
        dag.push(Node::new(this, self.deps.as_u64s(), "".into()));

        let mut md_paths = sort(&dag, this).map_err(|e| {
            let this = &self.this.0;
            match e {
                TopsortError::TargetNotFound(x) => {
                    anyhow!("Failed topolosorting {this}: {} not found", MdId::from_u64(x).0)
                }
                TopsortError::CyclicDependency(x) => {
                    anyhow!("Failed topolosorting {this}: cyclic {}", MdId::from_u64(x).0)
                }
            }
        })?;
        let last = md_paths.pop();
        assert_eq!(last.as_deref(), Some("".into()), "BUG: it's self.this's empty path");

        Ok(md_paths)
    }

    pub(crate) fn block_along_with_predecessors(&self, mds: &[Md]) -> String {
        let mut blocks = String::new();
        let mut visited_cratesio_stages = IndexSet::new();
        for md in mds {
            md.append_blocks(&mut blocks, &mut visited_cratesio_stages);
            blocks.push('\n');
            for line in toml::to_string_pretty(md).expect("previously enc").lines() {
                blocks.push_str("## ");
                blocks.push_str(line);
                blocks.push('\n');
            }
            blocks.push('\n');
        }
        self.append_blocks(&mut blocks, &mut visited_cratesio_stages);
        blocks
    }
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

#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq, Hash)]
struct MdId(String);

impl MdId {
    #[must_use]
    fn from_u64(metadata: u64) -> Self {
        Self(format!("{metadata:#x}").trim_start_matches("0x").to_owned())
    }

    #[must_use]
    fn as_u64(&self) -> u64 {
        u64::from_str_radix(self.0.as_ref(), 16).expect("16-digit hex str")
    }
}

trait MdIdsExt {
    #[must_use]
    fn as_u64s(&self) -> Vec<u64>;
}

impl MdIdsExt for IndexSet<MdId> {
    fn as_u64s(&self) -> Vec<u64> {
        self.into_iter().map(MdId::as_u64).collect()
    }
}

#[test]
fn dec_decs() {
    let as_hex = MdId("dab737da4696ee62".to_owned());
    let as_dec = 15760126831633034850;
    assert_eq!(as_hex.as_u64(), as_dec);
    assert_eq!(MdId::from_u64(as_dec), as_hex);
    assert_eq!(MdId::from_u64(MdId::from_u64(as_dec).as_u64()).as_u64(), as_dec);
}

#[test]
fn md_ser() {
    let md = Md {
        this: MdId("711ba64e1183a234".to_owned()),
        deps: [MdId("81529f4c2380d9ec".to_owned()), MdId("88a4324b2aff6db9".to_owned())].into(),
        short_externs: [
            "pico_args-b8c41dbf50ca5479".to_owned(),
            "shlex-96a741f581f4126a".to_owned(),
        ]
        .into(),
        is_proc_macro: true,
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
    "pico_args-b8c41dbf50ca5479",
    "shlex-96a741f581f4126a",
]
is_proc_macro = true
writes = [
    "deps/primeorder-06397107ab8300fa.d",
    "deps/libprimeorder-06397107ab8300fa.rmeta",
    "deps/libprimeorder-06397107ab8300fa.rlib",
]

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

    let this = MdId("9494aa6093cd94c9".to_owned());
    let deps = [MdId("0dc1fe2644e3176a".to_owned())].into();
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
    assert_eq!(md.this, this);
    assert_eq!(md.deps, deps);
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
