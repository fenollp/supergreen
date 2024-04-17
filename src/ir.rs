// Internal Representation utils

use std::{
    fs::File,
    io::{BufRead, BufReader},
};

use anyhow::{anyhow, Result};
use serde::Deserialize;

pub(crate) const CRATESIO_PREFIX: &str = "index.crates.io-";

pub(crate) const HDR: &str = "# ";

// # syntax = ..
// # this = "x"
// # mnt = ["y", "z"]
// # contexts = [
// #   { name = "a", uri = "b" },
// # ]
// FROM ..
#[derive(Debug, Deserialize, PartialEq)]
pub(crate) struct Head {
    #[serde(deserialize_with = "dec_id")]
    pub(crate) this: u64,
    #[serde(deserialize_with = "dec_ids")]
    pub(crate) mnt: Vec<u64>,
    pub(crate) contexts: Vec<BuildContext>,
}
impl Head {
    pub(crate) fn new(this: &str) -> Self {
        Self { this: dec(&this.to_owned()), mnt: vec![], contexts: vec![] }
    }

    pub(crate) fn from_file(fd: File) -> Result<Self> {
        toml::from_str(
            &BufReader::new(fd)
                .lines()
                .map_while(Result::ok)
                .take_while(|x| x.starts_with(HDR))
                .filter(|x| !x.starts_with("# syntax"))
                .map(|x| x.strip_prefix(HDR).unwrap_or(&x).to_owned())
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .map_err(|e| anyhow!("Parsing TOML head: {e}"))
    }

    pub(crate) fn write_to_slice(&self, header: &mut String) {
        let Self { this, mnt, contexts } = self;
        header.push_str(&format!("# this = {:?}\n", enc(*this)));
        let mnt: Vec<_> = mnt.iter().map(|x| format!("{:?}", enc(*x))).collect();
        header.push_str(&format!("# mnt = [{}]\n", mnt.join(",")));
        header.push_str("# contexts = [\n");
        for BuildContext { name, uri } in contexts {
            header.push_str(&format!("{HDR}  {{ name = {name:?}, uri = {uri:?} }},\n"));
        }
        header.push_str("# ]\n");
    }
}

#[inline]
fn dec(#[allow(clippy::ptr_arg)] x: &String) -> u64 {
    u64::from_str_radix(x, 16).expect(">>>")
}

#[inline]
fn enc(metadata: u64) -> String {
    format!("{metadata:#x}").trim_start_matches("0x").to_owned()
}

fn dec_id<'de, D>(de: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(dec(&<String>::deserialize(de)?))
}

fn dec_ids<'de, D>(de: D) -> Result<Vec<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    <Vec<String>>::deserialize(de)?.into_iter().map(|x| Ok(dec(&x))).collect()
}

#[test]
fn dec_decs() {
    let as_hex = "dab737da4696ee62".to_owned();
    let as_dec = 15760126831633034850;
    assert_eq!(dec(&as_hex), as_dec);
    assert_eq!(enc(as_dec), format!("{as_hex}"));
}

// FIXME: nutype?
#[inline]
#[must_use]
pub(crate) fn safe_stage(stage: String) -> String {
    stage
        .to_lowercase()
        .replace(|c: char| c != '.' && !c.is_alphanumeric(), "-")
        .replace("___", "_")
        .to_owned()
}

#[test]
fn safe_stages() {
    pretty_assertions::assert_eq!(
        safe_stage("libgit2-sys-0.14.2+1.5.1-index.crates.io-6f17d22bba15001f".to_owned()),
        "libgit2-sys-0.14.2-1.5.1-index.crates.io-6f17d22bba15001f".to_owned()
    );
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct BuildContext {
    pub(crate) name: String, // TODO: constrain with Docker stage name pattern
    pub(crate) uri: String,  // TODO: constrain with Docker build-context URIs
}
impl BuildContext {
    #[inline]
    #[must_use]
    pub(crate) fn is_readonly_mount(&self) -> bool {
        self.name.contains(CRATESIO_PREFIX) ||
        // TODO: create a stage from sources where able (public repos) use --secret mounts for private deps (and secreet direct artifacts)
        self.name.starts_with("input_") ||
         // TODO: link this to the build script it's coming from
         self.name.starts_with("crate_out-")

        // 0 0s release λ \grep -Eo from=crate_out-[^,]+ -r .|rev|cut -d- -f1|rev|sort -u
        // 265546c15e86ed0e
        // 32442d049a6a6273
        // 6730bb71af4c9e5e
        // 68773b12152d6b74
        // 7c50baf419c12613
        // 806d05c2cb9423e5
        // 890fbb16b2570e5a
        // c0cfb33e19b51a94
        // c21afc9aa144d6a6
        // 0 0s release λ ag 6730bb71af4c9e5e
        // num_traits-3ab0e20848896109-final.Dockerfile
        // 3:#   { name = "crate_out-6730bb71af4c9e5e", uri = "/tmp/clis-vixargs_0-1-0/release/build/num-traits-6730bb71af4c9e5e/out" },
        // 27:  OUT_DIR="/tmp/clis-vixargs_0-1-0/release/build/num-traits-6730bb71af4c9e5e/out" \
        // 32:  --mount=type=bind,from=crate_out-6730bb71af4c9e5e,target=/tmp/clis-vixargs_0-1-0/release/build/num-traits-6730bb71af4c9e5e/out \

        // vixargs-d2f27f94bee85c6b-final.Dockerfile
        // 5:#   { name = "crate_out-6730bb71af4c9e5e", uri = "/tmp/clis-vixargs_0-1-0/release/build/num-traits-6730bb71af4c9e5e/out" },
        // 331:  OUT_DIR="/tmp/clis-vixargs_0-1-0/release/build/num-traits-6730bb71af4c9e5e/out" \
        // 336:  --mount=type=bind,from=crate_out-6730bb71af4c9e5e,target=/tmp/clis-vixargs_0-1-0/release/build/num-traits-6730bb71af4c9e5e/out \

        // .fingerprint/num-traits-6730bb71af4c9e5e/run-build-script-build-script-build.json
        // 1:{"rustc":16286356497298320803,"features":"","declared_features":"","target":0,"profile":0,"path":0,"deps":[[3889717946063921280,"build_script_build",
        // false,10623348317785739830]],"local":[{"RerunIfChanged":{"output":"release/build/num-traits-6730bb71af4c9e5e/output","paths":["build.rs"]}}],"rustflags":[],"metadata":0,"config":0,"compile_kind":0}
    }
}
impl From<(String, String)> for BuildContext {
    fn from((name, uri): (String, String)) -> Self {
        Self { name, uri }
    }
}

#[test]
fn head_ser() {
    let head = Head {
        this: dec(&"711ba64e1183a234".to_owned()),
        mnt: vec![dec(&"81529f4c2380d9ec".to_owned()), dec(&"88a4324b2aff6db9".to_owned())],
        contexts: vec![BuildContext { name: "rust".to_owned(), uri: "docker-image://docker.io/library/rust:1.77.2-slim@sha256:090d8d4e37850b349b59912647cc7a35c6a64dba8168f6998562f02483fa37d7".to_owned() }],
    };

    let mut dst = String::new();
    head.write_to_slice(&mut dst);
    assert_eq!(
        dst,
        r#"# this = "711ba64e1183a234"
# mnt = ["81529f4c2380d9ec","88a4324b2aff6db9"]
# contexts = [
#   { name = "rust", uri = "docker-image://docker.io/library/rust:1.77.2-slim@sha256:090d8d4e37850b349b59912647cc7a35c6a64dba8168f6998562f02483fa37d7" },
# ]
"#
    );
}

#[test]
fn head_utils() {
    use std::fs;

    use mktemp::Temp;

    use crate::RUST;

    let tmp = Temp::new_file().unwrap();
    fs::write(&tmp, r#"# syntax=docker.io/docker/dockerfile:1
# this = "9494aa6093cd94c9"
# mnt = ["0dc1fe2644e3176a"]
# contexts = [
#   { name = "rust", uri = "docker-image://docker.io/library/rust:1.69.0-slim@sha256:8b85a8a6bf7ed968e24bab2eae6f390d2c9c8dbed791d3547fef584000f48f9e" },
#   { name = "input_src_lib_rs--rustversion-1.0.9", uri = "/home/maison/.cargo/registry/src/github.com-1ecc6299db9ec823/rustversion-1.0.9" },
#   { name = "crate_out-...", uri = "/home/maison/code/thing.git/target/debug/build/rustversion-ae69baa7face5565/out" },
# ]
...
"#).unwrap();

    let this = dec(&"9494aa6093cd94c9".to_owned());
    let mnt = vec![dec(&"0dc1fe2644e3176a".to_owned())];
    let contexts = vec![
        BuildContext {
    name:RUST.to_owned(),
    uri : "docker-image://docker.io/library/rust:1.69.0-slim@sha256:8b85a8a6bf7ed968e24bab2eae6f390d2c9c8dbed791d3547fef584000f48f9e".to_owned(),
        },
        BuildContext {
            name: "input_src_lib_rs--rustversion-1.0.9".to_owned(),
            uri: "/home/maison/.cargo/registry/src/github.com-1ecc6299db9ec823/rustversion-1.0.9"
                .to_owned(),
        },
        BuildContext {
            name: "crate_out-...".to_owned(),
            uri: "/home/maison/code/thing.git/target/debug/build/rustversion-ae69baa7face5565/out"
                .to_owned(),
        },
    ];
    let fd = File::open(tmp).unwrap();
    assert_eq!(Head::from_file(fd).unwrap(), Head { this, mnt, contexts: contexts.clone() });

    let used: Vec<_> = contexts.into_iter().filter(BuildContext::is_readonly_mount).collect();
    assert!(used[0].name.starts_with("input_"));
    assert!(used[1].name.starts_with("crate_out-"));
    assert_eq!(used.len(), 2);
}

#[test]
fn head_parsing_failure() {
    use std::fs;

    use mktemp::Temp;

    let tmp = Temp::new_file().unwrap();
    fs::write(&tmp, r#"# syntax=docker.io/docker/dockerfile:1@sha256:dbbd5e059e8a07ff7ea6233b213b36aa516b4c53c645f1817a4dd18b83cbea56
# this = "81529f4c2380d9ec"
# mnt = [[]]
# contexts = [
#   { name = "rust", uri = "docker-image://docker.io/library/rust:1.77.2-slim@sha256:090d8d4e37850b349b59912647cc7a35c6a64dba8168f6998562f02483fa37d7" },
# ]
FROM bla
"#).unwrap();

    let fd = File::open(tmp).unwrap();
    let err = Head::from_file(fd).err().map(|x| x.to_string()).unwrap_or_default();
    assert!(err.contains("\n2 | mnt = [[]]\n"));
    assert!(err.contains("\ninvalid type: sequence, expected a string\n"));
}
