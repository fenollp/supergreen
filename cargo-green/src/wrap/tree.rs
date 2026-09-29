//! A dependency tree grown one cargo call at a time, as in `recipes/vixargs@0.1.0.Dockerfile`:
//!
//! ```text
//! autocfg                     N  lib, from crates.io
//!  └─ num-traits/build.rs     X  its build script, compiled against autocfg
//!      └─ (running it)        Z  writes $OUT_DIR
//!          └─ num-traits      N  lib, compiled with that $OUT_DIR
//!              └─ mycrate     N  bin, local code
//! ```
//!
//! Each step is a real [`wrap_rustc`] or [`exec_build_script`] call. The only thing
//! faked is what the runner reports back; each step's Md is read by the next.

use std::sync::Arc;

use super::{Vars, exec_build_script, wrap_rustc};
use crate::{
    base_image::BaseImage,
    dirs::Paths,
    r#final::Final,
    green::Green,
    runner::Runner,
    sys::{
        Sys,
        fake::{FakeBuilds, FakeFs},
    },
};

const CARGO_HOME: &str = "/home/u/.cargo";
const SRC: &str = "/home/u/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f";
const CACHE: &str = "/home/u/.cargo/registry/cache/index.crates.io-1949cf8c6b5b557f";
const PWD: &str = "/home/u/mycrate";
const DEBUG: &str = "/home/u/mycrate/target/debug";
const RECIPE: &str = "/home/u/recipe.Dockerfile";

const AUTOCFG: &str = "1111111111111111";
const BUILT: &str = "2222222222222222";
const RUN: &str = "3333333333333333";
const NUM_TRAITS: &str = "4444444444444444";
const MYCRATE: &str = "5555555555555555";

struct Tree {
    fs: Arc<FakeFs>,
    /// `build_script_build`, or its legacy name.
    buildrs: &'static str,
    experiments: Vec<&'static str>,
}

impl Tree {
    fn new() -> Self {
        let fs = FakeFs::new();
        fs.file(format!("{CACHE}/autocfg-1.0.1.crate"), "<autocfg tarball>");
        fs.file(format!("{CACHE}/num-traits-0.2.14.crate"), "<num-traits tarball>");
        fs.pushd(PWD);
        fs.file("/Cargo.toml", "");
        fs.file("/src/main.rs", "");
        fs.file("/target/CACHEDIR.TAG", "");
        fs.popd();
        Self { fs, buildrs: "build_script_build", experiments: vec![] }
    }

    fn green(&self, vars: &[(&str, &str)]) -> Green {
        let env: Vars = vars.iter().map(|&(k, v)| (k.to_owned(), v.to_owned())).collect();
        Green {
            env,
            runner: Runner::Docker,
            base: BaseImage {
                image_inline: "FROM docker.io/library/rust:1.99.0-slim AS rust-base".to_owned(),
                ..Default::default()
            },
            r#final: Final { path: Some(RECIPE.into()) },
            experiment: ["finalpathnonprimary"]
                .iter()
                .chain(&self.experiments)
                .map(|&x| x.to_owned())
                .collect(),
            paths: Paths {
                cargo_home: CARGO_HOME.into(),
                cwd: PWD.into(),
                host_target_dir: Some(format!("{PWD}/target").into()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Runs `f` as if the runner then reported having written `writes` and, for build
    /// scripts, `cargo::rustc-env=`s `sets`. Returns the Containerfile it built.
    fn call(
        &self,
        sets: &[(&str, &str)],
        writes: &[&str],
        f: impl AsyncFnOnce() -> anyhow::Result<()>,
    ) -> String {
        let builds = FakeBuilds::with_written_and_set(sets.iter().copied(), writes);
        let _guard =
            Sys::install(Sys { fs: self.fs.clone(), builds: builds.clone(), ..Sys::fake() });
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f()).unwrap();
        let [built] = &builds.built()[..] else { panic!("expected one build") };
        self.fs.read(built).unwrap()
    }

    fn rustc(&self, vars: &[(&str, &str)], pwd: &str, args: &str, writes: &[&str]) -> String {
        let green = self.green(vars);
        let args = args.split_whitespace().map(ToOwned::to_owned).collect();
        self.call(&[], writes, async || {
            wrap_rustc(green, args, pwd.into(), async { unreachable!("no fallback") }).await
        })
    }

    /// The Md of `mdid`, minus its stages (the Containerfile shows those).
    fn md(&self, mdid: &str) -> String {
        let md = self.fs.read(format!("{DEBUG}/{mdid}.toml")).unwrap();
        md.split_once("\n[[stages]]").unwrap().0.to_owned()
    }

    fn autocfg(&self) -> String {
        let dir = format!("{SRC}/autocfg-1.0.1");
        self.rustc(
            &[
                ("CARGO_CRATE_NAME", "autocfg"),
                ("CARGO_MANIFEST_DIR", &dir),
                ("CARGO_PKG_NAME", "autocfg"),
                ("CARGO_PKG_VERSION", "1.0.1"),
            ],
            &dir,
            &format!(
                "--crate-name autocfg --edition=2015 {dir}/src/lib.rs --crate-type lib \
                 --emit=dep-info,metadata,link -C extra-filename=-{AUTOCFG} \
                 --out-dir {DEBUG}/deps -L dependency={DEBUG}/deps"
            ),
            &[
                &format!("autocfg-{AUTOCFG}.d"),
                &format!("libautocfg-{AUTOCFG}.rlib"),
                &format!("libautocfg-{AUTOCFG}.rmeta"),
            ],
        )
    }

    fn num_traits_buildrs(&self) -> String {
        let dir = format!("{SRC}/num-traits-0.2.14");
        let name = self.buildrs;
        self.rustc(
            &[
                ("CARGO_CRATE_NAME", name),
                ("CARGO_MANIFEST_DIR", &dir),
                ("CARGO_PKG_NAME", "num-traits"),
                ("CARGO_PKG_VERSION", "0.2.14"),
            ],
            &dir,
            &format!(
                "--crate-name {name} --edition=2015 {dir}/build.rs --crate-type bin \
                 --emit=dep-info,link -C extra-filename=-{BUILT} \
                 --out-dir {DEBUG}/build/num-traits-{BUILT} -L dependency={DEBUG}/deps \
                 --extern autocfg={DEBUG}/deps/libautocfg-{AUTOCFG}.rlib"
            ),
            &[&format!("{name}-{BUILT}"), &format!("{name}-{BUILT}.d")],
        )
    }

    fn num_traits_run(&self, writes: &[&str]) -> String {
        let green = self.green(&[
            ("CARGO_MANIFEST_DIR", &format!("{SRC}/num-traits-0.2.14")),
            ("CARGO_PKG_NAME", "num-traits"),
            ("CARGO_PKG_VERSION", "0.2.14"),
            ("OUT_DIR", &format!("{DEBUG}/build/num-traits-{RUN}/out")),
            ("TARGET", "x86_64-unknown-linux-gnu"),
            ("NUM_JOBS", "32"),
        ]);
        let exe = format!("{DEBUG}/build/num-traits-{BUILT}/{}", self.buildrs.replace('_', "-"));
        self.call(&[("NUM_TRAITS_PROBED", "1")], writes, async || {
            exec_build_script(green, exe.into()).await
        })
    }

    fn num_traits(&self) -> String {
        let dir = format!("{SRC}/num-traits-0.2.14");
        self.rustc(
            &[
                ("CARGO_CRATE_NAME", "num_traits"),
                ("CARGO_MANIFEST_DIR", &dir),
                ("CARGO_PKG_NAME", "num-traits"),
                ("CARGO_PKG_VERSION", "0.2.14"),
                ("OUT_DIR", &format!("{DEBUG}/build/num-traits-{RUN}/out")),
            ],
            &dir,
            &format!(
                "--crate-name num_traits --edition=2015 {dir}/src/lib.rs --crate-type lib \
                 --emit=dep-info,metadata,link -C extra-filename=-{NUM_TRAITS} \
                 --out-dir {DEBUG}/deps -L dependency={DEBUG}/deps --cfg has_i128"
            ),
            &[
                &format!("num_traits-{NUM_TRAITS}.d"),
                &format!("libnum_traits-{NUM_TRAITS}.rlib"),
                &format!("libnum_traits-{NUM_TRAITS}.rmeta"),
            ],
        )
    }

    fn mycrate(&self) -> String {
        self.rustc(
            &[
                ("CARGO_CRATE_NAME", "mycrate"),
                ("CARGO_MANIFEST_DIR", PWD),
                ("CARGO_PKG_NAME", "mycrate"),
                ("CARGO_PKG_VERSION", "0.1.0"),
            ],
            PWD,
            &format!(
                "--crate-name mycrate --edition=2024 src/main.rs --crate-type bin \
                 --emit=dep-info,link -C extra-filename=-{MYCRATE} \
                 --out-dir {DEBUG}/deps -L dependency={DEBUG}/deps \
                 --extern num_traits={DEBUG}/deps/libnum_traits-{NUM_TRAITS}.rlib"
            ),
            &[&format!("mycrate-{MYCRATE}"), &format!("mycrate-{MYCRATE}.d")],
        )
    }

    /// The whole tree, returning mycrate's Containerfile.
    fn grow(&self) -> String {
        self.autocfg();
        self.num_traits_buildrs();
        self.num_traits_run(&["out/probe.rs"]);
        self.num_traits();
        self.mycrate()
    }
}

/// What `child` adds to the Containerfile of its last dependency, `parent`: a crate's
/// Containerfile always extends those of its dependencies.
fn grown<'a>(parent: &str, child: &'a str) -> &'a str {
    child.strip_prefix(parent).unwrap_or_else(|| panic!("{child}\n\ndoes not extend\n\n{parent}"))
}

#[test]
fn a_registry_crate() {
    let t = Tree::new();
    assert_snapshots_eq!(
        t.autocfg(),
        snapbox::str![[r#"
# syntax=docker.io/docker/dockerfile:1
# check=error=true
# Generated by https://github.com/fenollp/supergreen v[..]

FROM docker.io/library/rust:1.99.0-slim AS rust-base
ARG SOURCE_DATE_EPOCH=42


FROM scratch AS cratesio-autocfg-1.0.1
ADD --unpack --checksum=sha256:7274133f6411f485cc9448b5b7e5dd050704d840e6987ed62bbf02aec0bd7e22 \
  https://static.crates.io/crates/autocfg/autocfg-1.0.1.crate /
FROM rust-base AS dep-n-autocfg-1.0.1-1111111111111111
WORKDIR /target/debug/deps
RUN \
  --mount=from=cratesio-autocfg-1.0.1,source=/autocfg-1.0.1,dst=$CARGO_HOME/registry/src/index.crates.io/autocfg-1.0.1 \
    env CARGO_CRATE_NAME=autocfg \
        CARGO_MANIFEST_DIR=$CARGO_HOME/registry/src/index.crates.io/autocfg-1.0.1 \
        CARGO_PKG_NAME=autocfg \
        CARGO_PKG_VERSION=1.0.1 \
        CARGOGREEN=1 \
      rustc --crate-name autocfg --crate-type lib --edition 2015 --emit dep-info,metadata,link --out-dir /target/debug/deps -C extra-filename'=-1111111111111111' -L dependency'=/target/debug/deps' $CARGO_HOME/registry/src/index.crates.io/autocfg-1.0.1/src/lib.rs \
        1>          ../out-1111111111111111-stdout \
        2>          ../out-1111111111111111-stderr \
        || echo $? >../out-1111111111111111-errcode\
  ; find ./ ../out-1111111111111111-* -name '*-1111111111111111*' -exec touch --no-dereference --date=@$SOURCE_DATE_EPOCH '{}' + \
 || echo $? >../out-1111111111111111-errcode
FROM scratch AS out-1111111111111111
COPY --link --from=dep-n-autocfg-1.0.1-1111111111111111 /target/debug/deps /deps
COPY --link --from=dep-n-autocfg-1.0.1-1111111111111111 /target/debug/out-1111111111111111-* /

"#]]
    );
    assert_snapshots_eq!(
        t.md(AUTOCFG),
        snapbox::str![[r#"
stamp = 1
this = "1111111111111111"
writes = [
    "autocfg-1111111111111111.d",
    "libautocfg-1111111111111111.rlib",
    "libautocfg-1111111111111111.rmeta",
]

"#]]
    );
}

/// Compiling a build script is compiling a crate, with the `exe_dance` at the end.
#[test]
fn a_build_script_with_a_dependency() {
    let t = Tree::new();
    let parent = t.autocfg();
    assert_snapshots_eq!(
        grown(&parent, &t.num_traits_buildrs()),
        snapbox::str![[r#"

FROM scratch AS cratesio-num-traits-0.2.14
ADD --unpack --checksum=sha256:7d4fbc5f94ad99bfbc7b2308db820997421362673cd9c55ce03c15ceaa1e37aa \
  https://static.crates.io/crates/num-traits/num-traits-0.2.14.crate /
FROM rust-base AS dep-x-num-traits-0.2.14-2222222222222222
WORKDIR /target/debug/build/num-traits-2222222222222222
RUN \
  --mount=from=cratesio-num-traits-0.2.14,source=/num-traits-0.2.14,dst=$CARGO_HOME/registry/src/index.crates.io/num-traits-0.2.14 \
  --mount=from=out-1111111111111111,dst=/target/debug/deps/libautocfg-1111111111111111.rlib,source=/deps/libautocfg-1111111111111111.rlib \
    env CARGO_CRATE_NAME=build_script_build \
        CARGO_MANIFEST_DIR=$CARGO_HOME/registry/src/index.crates.io/num-traits-0.2.14 \
        CARGO_PKG_NAME=num-traits \
        CARGO_PKG_VERSION=0.2.14 \
        CARGOGREEN=1 \
      rustc --crate-name build_script_build --crate-type bin --edition 2015 --emit dep-info,link --extern autocfg'=/target/debug/deps/libautocfg-1111111111111111.rlib' --out-dir /target/debug/build/num-traits-2222222222222222 -C extra-filename'=-2222222222222222' -L dependency'=/target/debug/deps' $CARGO_HOME/registry/src/index.crates.io/num-traits-0.2.14/build.rs \
        1>          ../out-2222222222222222-stdout \
        2>          ../out-2222222222222222-stderr \
        || echo $? >../out-2222222222222222-errcode\
  ; mv ./build_script_build-2222222222222222 ./_build_script_build-2222222222222222 \
 && printf '#!/bin/sh\nenv CARGOGREEN_EXECUTEBUILDSCRIPT=$0 cargo-green\n' >./build_script_build-2222222222222222 \
 && chmod +x ./build_script_build-2222222222222222 \
 || echo $? >../out-2222222222222222-errcode \
  ; find ./ ../out-2222222222222222-* -exec touch --no-dereference --date=@$SOURCE_DATE_EPOCH '{}' + \
 || echo $? >../out-2222222222222222-errcode
FROM scratch AS out-2222222222222222
COPY --link --from=dep-x-num-traits-0.2.14-2222222222222222 /target/debug/build/num-traits-2222222222222222 /num-traits-2222222222222222
COPY --link --from=dep-x-num-traits-0.2.14-2222222222222222 /target/debug/build/out-2222222222222222-* /

"#]]
    );
    assert_snapshots_eq!(
        t.md(BUILT),
        snapbox::str![[r#"
stamp = 1
this = "2222222222222222"
deps = ["1111111111111111"]
buildrs = true
writes = [
    "build_script_build-2222222222222222",
    "build_script_build-2222222222222222.d",
]

[[externs]]
name = "out-1111111111111111"
mount = "libautocfg-1111111111111111.rlib"

"#]]
    );
}

/// Runs where cargo would, in the crate's source, and records its `$OUT_DIR`.
#[test]
fn running_a_build_script() {
    let t = Tree::new();
    t.autocfg();
    let parent = t.num_traits_buildrs();
    assert_snapshots_eq!(
        grown(&parent, &t.num_traits_run(&["out/probe.rs"])),
        snapbox::str![[r#"

FROM rust-base AS run-z-num-traits-0.2.14-3333333333333333
WORKDIR /target/debug/build/num-traits-3333333333333333/out
WORKDIR $CARGO_HOME/registry/src/index.crates.io/num-traits-0.2.14
RUN \
  --mount=from=out-2222222222222222,source=/num-traits-2222222222222222/_build_script_build-2222222222222222,dst=/target/debug/build/num-traits-2222222222222222/build-script-build \
  --mount=from=cratesio-num-traits-0.2.14,source=/num-traits-0.2.14,dst=$CARGO_HOME/registry/src/index.crates.io/num-traits-0.2.14 \
    env CARGO_MANIFEST_DIR=$CARGO_HOME/registry/src/index.crates.io/num-traits-0.2.14 \
        CARGO_PKG_NAME=num-traits \
        CARGO_PKG_VERSION=0.2.14 \
        NUM_JOBS=1 \
        OUT_DIR=/target/debug/build/num-traits-3333333333333333/out \
        TARGET=x86_64-unknown-linux-gnu \
        CARGOGREEN=1 \
      /target/debug/build/num-traits-2222222222222222/build-script-build \
        1>          /target/debug/build/num-traits-3333333333333333/out-3333333333333333-stdout \
        2>          /target/debug/build/num-traits-3333333333333333/out-3333333333333333-stderr \
        || echo $? >/target/debug/build/num-traits-3333333333333333/out-3333333333333333-errcode\
  ; find /target/debug/build/num-traits-3333333333333333/out/ /target/debug/build/num-traits-3333333333333333/out-3333333333333333-* -exec touch --no-dereference --date=@$SOURCE_DATE_EPOCH '{}' + \
 || echo $? >/target/debug/build/num-traits-3333333333333333/out-3333333333333333-errcode
FROM scratch AS out-3333333333333333
COPY --link --from=run-z-num-traits-0.2.14-3333333333333333 /target/debug/build/num-traits-3333333333333333/out /out
COPY --link --from=run-z-num-traits-0.2.14-3333333333333333 /target/debug/build/num-traits-3333333333333333/out-3333333333333333-* /

"#]]
    );
    assert_snapshots_eq!(
        t.md(RUN),
        snapbox::str![[r#"
stamp = 1
this = "3333333333333333"
deps = [
    "1111111111111111",
    "2222222222222222",
]
buildrs = true
writes_to = "/target/debug/build/num-traits-3333333333333333/out"
writes = ["out/probe.rs"]

[set_envs]
NUM_TRAITS_PROBED = "1"

"#]]
    );
}

/// The crate gets its build script's `$OUT_DIR` mounted and its `rustc-env`s set.
#[test]
fn a_crate_compiled_with_its_out_dir() {
    let t = Tree::new();
    t.autocfg();
    t.num_traits_buildrs();
    let parent = t.num_traits_run(&["out/probe.rs"]);
    assert_snapshots_eq!(
        grown(&parent, &t.num_traits()),
        snapbox::str![[r#"


FROM rust-base AS dep-n-num-traits-0.2.14-4444444444444444
WORKDIR /target/debug/deps
RUN \
  --mount=from=cratesio-num-traits-0.2.14,source=/num-traits-0.2.14,dst=$CARGO_HOME/registry/src/index.crates.io/num-traits-0.2.14 \
  --mount=from=out-3333333333333333,dst=/target/debug/build/num-traits-3333333333333333/out,source=/out \
    env CARGO_CRATE_NAME=num_traits \
        CARGO_MANIFEST_DIR=$CARGO_HOME/registry/src/index.crates.io/num-traits-0.2.14 \
        CARGO_PKG_NAME=num-traits \
        CARGO_PKG_VERSION=0.2.14 \
        OUT_DIR=/target/debug/build/num-traits-3333333333333333/out \
        CARGOGREEN=1 \
        NUM_TRAITS_PROBED=1 \
      rustc --cfg has_i128 --crate-name num_traits --crate-type lib --edition 2015 --emit dep-info,metadata,link --out-dir /target/debug/deps -C extra-filename'=-4444444444444444' -L dependency'=/target/debug/deps' $CARGO_HOME/registry/src/index.crates.io/num-traits-0.2.14/src/lib.rs \
        1>          ../out-4444444444444444-stdout \
        2>          ../out-4444444444444444-stderr \
        || echo $? >../out-4444444444444444-errcode\
  ; find ./ ../out-4444444444444444-* -name '*-4444444444444444*' -exec touch --no-dereference --date=@$SOURCE_DATE_EPOCH '{}' + \
 || echo $? >../out-4444444444444444-errcode
FROM scratch AS out-4444444444444444
COPY --link --from=dep-n-num-traits-0.2.14-4444444444444444 /target/debug/deps /deps
COPY --link --from=dep-n-num-traits-0.2.14-4444444444444444 /target/debug/out-4444444444444444-* /

"#]]
    );
    assert_snapshots_eq!(
        t.md(NUM_TRAITS),
        snapbox::str![[r#"
stamp = 1
this = "4444444444444444"
deps = [
    "1111111111111111",
    "2222222222222222",
    "3333333333333333",
]
buildrs_results = ["3333333333333333"]
writes = [
    "num_traits-4444444444444444.d",
    "libnum_traits-4444444444444444.rlib",
    "libnum_traits-4444444444444444.rmeta",
]

[[mounts]]
name = "out-3333333333333333"
mount = "/target/debug/build/num-traits-3333333333333333/out"

"#]]
    );
}

/// Local code comes from a build context, remapped under `/work`.
#[test]
fn a_local_crate_on_top() {
    let t = Tree::new();
    t.autocfg();
    t.num_traits_buildrs();
    t.num_traits_run(&["out/probe.rs"]);
    let parent = t.num_traits();
    assert_snapshots_eq!(
        grown(&parent, &t.mycrate()),
        snapbox::str![[r#"

FROM rust-base AS dep-n-mycrate-0.1.0-5555555555555555
WORKDIR /target/debug/deps
WORKDIR /work
RUN \
  --mount=from=cwd-5555555555555555,dst=/work/Cargo.toml,source=/Cargo.toml \
  --mount=from=cwd-5555555555555555,dst=/work/src,source=/src \
  --mount=from=out-4444444444444444,dst=/target/debug/deps/libnum_traits-4444444444444444.rlib,source=/deps/libnum_traits-4444444444444444.rlib \
  --mount=from=out-1111111111111111,dst=/target/debug/deps/libautocfg-1111111111111111.rlib,source=/deps/libautocfg-1111111111111111.rlib \
  --mount=from=out-3333333333333333,dst=/target/debug/build/num-traits-3333333333333333/out,source=/out \
    env CARGO_CRATE_NAME=mycrate \
        CARGO_MANIFEST_DIR=/work \
        CARGO_PKG_NAME=mycrate \
        CARGO_PKG_VERSION=0.1.0 \
        CARGOGREEN=1 \
      rustc --crate-name mycrate --crate-type bin --edition 2024 --emit dep-info,link --extern num_traits'=/target/debug/deps/libnum_traits-4444444444444444.rlib' --out-dir /target/debug/deps -C extra-filename'=-5555555555555555' -L dependency'=/target/debug/deps' src/main.rs \
        1>          /target/debug/out-5555555555555555-stdout \
        2>          /target/debug/out-5555555555555555-stderr \
        || echo $? >/target/debug/out-5555555555555555-errcode\
  ; find /target/debug/deps/ /target/debug/out-5555555555555555-* -name '*-5555555555555555*' -exec touch --no-dereference --date=@$SOURCE_DATE_EPOCH '{}' + \
 || echo $? >/target/debug/out-5555555555555555-errcode
FROM scratch AS out-5555555555555555
COPY --link --from=dep-n-mycrate-0.1.0-5555555555555555 /target/debug/deps /deps
COPY --link --from=dep-n-mycrate-0.1.0-5555555555555555 /target/debug/out-5555555555555555-* /

"#]]
    );
    assert_snapshots_eq!(
        t.md(MYCRATE),
        snapbox::str![[r#"
stamp = 1
this = "5555555555555555"
deps = [
    "1111111111111111",
    "2222222222222222",
    "3333333333333333",
    "4444444444444444",
]
buildrs_results = ["3333333333333333"]
writes = [
    "mycrate-5555555555555555",
    "mycrate-5555555555555555.d",
]

[[externs]]
name = "out-4444444444444444"
mount = "libnum_traits-4444444444444444.rlib"

[[externs]]
name = "out-1111111111111111"
mount = "libautocfg-1111111111111111.rlib"

[[mounts]]
name = "out-3333333333333333"
mount = "/target/debug/build/num-traits-3333333333333333/out"

[[contexts]]
name = "cwd-5555555555555555"
uri = "/home/u/mycrate"

"#]]
    );
    assert_snapshots_eq!(
        t.fs.read(RECIPE).unwrap().split_once("\n# Pipe").unwrap().1,
        snapbox::str![[r#"
 this file to (not portable due to usage of local build contexts):
# DOCKER_BUILDKIT="1" \
#   docker buildx build --target=out-5555555555555555 <THIS_FILE

FROM scratch
COPY --link --from=out-5555555555555555 /deps/mycrate-5555555555555555 /mycrate

"#]]
    );
}

/// Stages shared along the tree are written once: the base, and each crate's `ADD`.
#[test]
fn remote_stages_are_emitted_once() {
    let containerfile = Tree::new().grow();
    for (stage, count) in [
        ("AS rust-base", 1),
        ("AS cratesio-autocfg-1.0.1", 1),
        ("AS cratesio-num-traits-0.2.14", 1),
        ("FROM rust-base AS ", 5),
        ("FROM scratch AS out-", 5),
    ] {
        assert_eq!(containerfile.matches(stage).count(), count, "{stage:?} in {containerfile}");
    }
}

/// FIXME: a dependency's build-dependencies are mounted into its dependents, which never
/// link them. (corpus: vixargs's own stage mounts `libautocfg` and `libversion_check`)
#[test]
fn build_dependencies_leak_into_dependents() {
    let containerfile = Tree::new().grow();
    let own_stage = containerfile.split_once("AS dep-n-mycrate").unwrap().1;
    assert!(own_stage.contains(&format!("  --mount=from=out-{AUTOCFG},")), "{own_stage}");
}

/// Transitively, only build scripts that wrote something have their `$OUT_DIR` mounted.
#[test]
fn an_empty_out_dir_is_not_mounted_transitively() {
    let mount = format!("  --mount=from=out-{RUN},");

    let t = Tree::new();
    t.autocfg();
    t.num_traits_buildrs();
    t.num_traits_run(&[]);
    let parent = t.num_traits();
    assert!(parent.contains(&mount), "{parent}");
    let mycrate = t.mycrate();
    assert!(!grown(&parent, &mycrate).contains(&mount), "{mycrate}");
}

/// With `buildscriptsources`, a build script also sees its dependencies' sources, for
/// scripts that read files shipped in a dependency's tarball (eg. `protoc-bin-vendored`).
#[test]
fn buildscriptsources_mounts_dependencies_sources() {
    let mount = "  --mount=from=cratesio-autocfg-1.0.1,source=/autocfg-1.0.1,dst=$CARGO_HOME/registry/src/index.crates.io/autocfg-1.0.1 \\\n";
    for (experiments, mounted) in [(vec![], false), (vec!["buildscriptsources"], true)] {
        let mut t = Tree::new();
        t.experiments = experiments;
        t.autocfg();
        let parent = t.num_traits_buildrs();
        let run = t.num_traits_run(&["out/probe.rs"]);
        assert_eq!(grown(&parent, &run).contains(mount), mounted, "{run}");
    }
}

/// Cargo still names some build scripts `build_script_main`. (corpus: typenum, in cargo-authors)
#[test]
fn a_legacy_build_script_name() {
    let mut t = Tree::new();
    t.buildrs = "build_script_main";
    t.autocfg();
    assert!(t.num_traits_buildrs().contains(&format!(
        "  ; mv ./build_script_main-{BUILT} ./_build_script_main-{BUILT} \\\n"
    )));
    assert!(t.num_traits_run(&["out/probe.rs"]).contains(&format!(
        "  --mount=from=out-{BUILT},source=/num-traits-{BUILT}/_build_script_main-{BUILT},dst=/target/debug/build/num-traits-{BUILT}/build-script-main \\\n"
    )));
}

/// FIXME: a local crate's build script gets its sources mounted at their host path, but
/// runs from `/work`. (Not in the corpus: it only holds crates.io and git dependencies.)
#[test]
fn a_local_build_script_mounts_host_paths() {
    const LOCAL_BUILT: &str = "6666666666666666";
    let t = Tree::new();
    t.fs.file(format!("{PWD}/build.rs"), "");
    let vars = [
        ("CARGO_CRATE_NAME", "build_script_build"),
        ("CARGO_MANIFEST_DIR", PWD),
        ("CARGO_PKG_NAME", "mycrate"),
        ("CARGO_PKG_VERSION", "0.1.0"),
    ];
    t.rustc(
        &vars,
        PWD,
        &format!(
            "--crate-name build_script_build --edition=2024 build.rs --crate-type bin \
             --emit=dep-info,link -C extra-filename=-{LOCAL_BUILT} \
             --out-dir {DEBUG}/build/mycrate-{LOCAL_BUILT} -L dependency={DEBUG}/deps"
        ),
        &[&format!("build_script_build-{LOCAL_BUILT}")],
    );
    let green = t.green(&[
        ("CARGO_MANIFEST_DIR", PWD),
        ("CARGO_PKG_NAME", "mycrate"),
        ("CARGO_PKG_VERSION", "0.1.0"),
        ("OUT_DIR", &format!("{DEBUG}/build/mycrate-{MYCRATE}/out")),
    ]);
    let exe = format!("{DEBUG}/build/mycrate-{LOCAL_BUILT}/build-script-build");
    let run = t.call(&[], &["out/x"], async || exec_build_script(green, exe.into()).await);
    let run = run.split_once("AS run-").unwrap().1;
    assert!(run.contains("\nWORKDIR /work\n"), "{run}");
    assert!(run.contains(",dst=/home/u/mycrate/build.rs,source=/build.rs \\\n"), "{run}");
}
