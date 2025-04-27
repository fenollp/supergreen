// Our own MetaData utils

use std::{collections::BTreeSet, str::FromStr};

use anyhow::{bail, Result};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use szyk::{sort, Node};

use crate::{
    checkouts::CHECKOUTS_STAGE_PREFIX,
    cratesio::CRATESIO_STAGE_PREFIX,
    stage::{Stage, RST, RUST},
};

#[cfg_attr(test, derive(Default))]
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Md {
    pub(crate) this: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) deps: Vec<String>,

    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) contexts: BTreeSet<BuildContext>,

    stages: Vec<NamedStage>,
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
        Self { this: this.to_owned(), deps: vec![], contexts: [].into(), stages: [].into() }
    }

    pub(crate) fn to_string_pretty(&self) -> Result<String> {
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
        self.stages.push(NamedStage { name: name.clone(), script: block.trim().to_owned() });
    }

    fn append_blocks(&self, dockerfile: &mut String, visited: &mut BTreeSet<Stage>) {
        let mut stages = self.stages.iter().filter(|NamedStage { name, .. }| *name != *RUST);

        let NamedStage { name, script } = stages.next().expect("at least one stage");

        let mut filter = None;
        if name.starts_with(CHECKOUTS_STAGE_PREFIX) || name.starts_with(CRATESIO_STAGE_PREFIX) {
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

    pub(crate) fn extend_from_externs(
        &mut self,
        mds: Vec<(Utf8PathBuf, Self)>,
    ) -> Result<Vec<Utf8PathBuf>> {
        let mut dag: Vec<_> = mds
            .into_iter()
            .map(|(md_path, md)| {
                let this = dec(&md.this);
                self.deps.push(md.this);
                self.contexts
                    .extend(md.contexts.into_iter().filter(BuildContext::is_readonly_mount));
                Node::new(this, decs(&md.deps), md_path)
            })
            .collect();
        let this = dec(&self.this);
        dag.push(Node::new(this, decs(&self.deps), "".into()));

        let mut md_paths = match sort(&dag, this) {
            Ok(ordering) => ordering,
            Err(e) => bail!("Failed topolosorting {}: {e:?}", self.this),
        };
        md_paths.truncate(md_paths.len() - 1); // pop last (it's self.this's empty path)

        Ok(md_paths)
    }

    pub(crate) fn block_along_with_predecessors(&self, mds: &[Md]) -> String {
        let mut blocks = String::new();
        let mut visited_cratesio_stages = BTreeSet::new();
        for md in mds {
            md.append_blocks(&mut blocks, &mut visited_cratesio_stages);
            blocks.push('\n');
        }
        self.append_blocks(&mut blocks, &mut visited_cratesio_stages);
        blocks
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct NamedStage {
    name: Stage,
    script: String,
}

// pub(crate) const HDR: &str = "# ";

// # syntax = ..
// # this = "x"
// # mnt = ["y", "z"]
// # contexts = [
// #   { name = "a", uri = "b" },
// # ]
// FROM ..

#[must_use]
fn dec(x: impl AsRef<str>) -> u64 {
    u64::from_str_radix(x.as_ref(), 16).expect("16-digit hex str")
}

#[must_use]
fn decs(xs: &[String]) -> Vec<u64> {
    xs.iter().map(dec).collect()
}

#[test]
fn dec_decs() {
    fn enc(metadata: u64) -> String {
        format!("{metadata:#x}").trim_start_matches("0x").to_owned()
    }

    let as_hex = "dab737da4696ee62".to_owned();
    let as_dec = 15760126831633034850;
    assert_eq!(dec(&as_hex), as_dec);
    assert_eq!(enc(as_dec), format!("{as_hex}"));
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct BuildContext {
    pub(crate) name: Stage,
    /// Actually any BuildKit ctx works, we just only use local paths.
    pub(crate) uri: Utf8PathBuf,
}

impl BuildContext {
    #[must_use]
    pub(crate) fn is_readonly_mount(&self) -> bool {
        [
            CRATESIO_STAGE_PREFIX,
            "cwd-",
            "crate_out-", // TODO: link this to the build script it's coming from
        ]
        .iter()
        .any(|prefix| self.name.starts_with(prefix))
    }
}

#[test]
fn md_ser() {
    let md = Md {
        this: "711ba64e1183a234".to_owned(),
        deps: vec!["81529f4c2380d9ec".to_owned(), "88a4324b2aff6db9".to_owned()],
        contexts: [BuildContext {
            name: "rust".try_into().unwrap(),
            uri: "/some/local/path".into(),
        }]
        .into(),
        stages: vec![NamedStage { name: RUST.clone(), script: format!("FROM rust AS {RST}") }],
    };

    pretty_assertions::assert_eq!(
        r#"
this = "711ba64e1183a234"
deps = [
    "81529f4c2380d9ec",
    "88a4324b2aff6db9",
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

    let this = "9494aa6093cd94c9".to_owned();
    let deps = vec!["0dc1fe2644e3176a".to_owned()];
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

    let used: Vec<_> = contexts.into_iter().filter(BuildContext::is_readonly_mount).collect();
    dbg!(&used);
    assert_eq!(used.len(), 2);
    assert_eq!(&used[0].name[..10], "crate_out-");
    assert_eq!(&used[1].name[..4], "cwd-");
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
