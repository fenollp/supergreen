use std::collections::HashSet;

use anyhow::{Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use log::{error, info, trace};

use crate::{
    PKG, VSN,
    all_our_envs::OUT_DIR,
    cache::result::result_key,
    green::Green,
    logging::{self},
    md::{Md, MdId},
    stage::{AsStage, RST, RUST, Stage},
    sys::sys,
    wrap::{Vars, Wrapped, call_config},
};

const BUILDRS_NAME: &str = "build_script_build";
const BUILDRS_LEGACY: &str = "build_script_main";

#[must_use]
pub(crate) fn is_buildrs_executable(name: &str) -> bool {
    [BUILDRS_NAME, BUILDRS_LEGACY].contains(&name)
}

// NOTE: "build_script_build" vs "build_script_main": cargo's fight with legacy.
// NOTE: "build_script_build", "build-script-build" also Windows adds ".exe".
// TODO: one trick even further: pull a quine: a Shell script that calls to PKG
//       but still manages to embed the whole compiled build script. Thus leaving
//       only one file.
#[must_use]
pub(crate) fn exe_dance(mdid: MdId, crate_name: &str, out_dir: &Utf8Path) -> String {
    format!(
        r#"
  ; mv {out_dir}/{crate_name}-{mdid} {out_dir}/_{crate_name}-{mdid} \
 && printf '#!/bin/sh\nenv {var}=$0 {PKG}\n' >{out_dir}/{crate_name}-{mdid} \
 && chmod +x {out_dir}/{crate_name}-{mdid} \
"#,
        var = CARGOGREEN_EXECUTEBUILDSCRIPT!(),
    )[1..]
        .to_owned()
}

pub(crate) async fn exec_build_script(green: Green, exe: Utf8PathBuf, vars: &Vars) -> Result<()> {
    let (crate_name, pkg_name, pkg_version, pkg_manifest_dir) = call_config(vars);

    // exe: /target/release/build/proc-macro2-2f938e044e3f79bf/build-script-build
    let Some((previous_mdid, target_path)) = || -> Option<_> {
        // target_path: /target/release/build/proc-macro2-2f938e044e3f79bf
        let target_path = exe.parent()?;
        // mdid: 2f938e044e3f79bf
        let mdid: MdId = target_path.file_name()?.rsplit('-').next()?.into();
        // target_path: /target/release
        let target_path = target_path.parent()?.parent()?.to_owned();
        Some((mdid, target_path))
    }() else {
        bail!("BUG: malformed buildrs exe {exe:?}")
    };

    // $OUT_DIR: /target/release/build/proc-macro2-b97492fdd0201a99/out
    let out_dir_var: Utf8PathBuf = vars.get(OUT_DIR!()).expect(OUT_DIR).into();
    let Some(mdid) = || -> Option<_> {
        // name: proc-macro2-b97492fdd0201a99
        let name = out_dir_var.parent()?.file_name()?;
        // mdid: b97492fdd0201a99
        let mdid: MdId = name.rsplit('-').next()?.into();
        Some(mdid)
    }() else {
        bail!("BUG: malformed {OUT_DIR}={out_dir_var:?}")
    };

    // Z: for eggZecuting build scripts
    let full_pkg_id = format!("Z {pkg_name} {pkg_version}-{mdid}");
    logging::setup(&full_pkg_id);

    info!("{PKG}@{VSN} original args: {exe:?} green={green:?}");

    let wrapped = do_exec(
        green,
        crate_name.as_deref(),
        &pkg_name,
        &pkg_manifest_dir,
        full_pkg_id.replace(' ', "-"),
        vars,
        out_dir_var,
        exe,
        target_path,
        previous_mdid,
        mdid,
    )
    .await;

    match wrapped {
        Ok(Wrapped::Done) => Ok(()),
        // Running a build script is not something we can hand back to cargo: it expects
        // `$OUT_DIR` to have been filled in by the script it asked us to run.
        Ok(Wrapped::Fallback) => todo!("fallback()"),
        Err(e) => {
            error!("Error: {e}");
            Err(e)
        }
    }
}

#[expect(clippy::too_many_arguments)]
async fn do_exec(
    green: Green,
    crate_name: Option<&str>,
    pkg_name: &str,
    pkg_manifest_dir: &Utf8Path,
    crate_id: String,
    vars: &Vars,
    out_dir_var: Utf8PathBuf,
    exe: Utf8PathBuf,
    target_path: Utf8PathBuf,
    previous_mdid: MdId,
    mdid: MdId,
) -> Result<Wrapped> {
    let mut md: Md = mdid.into();
    md.build_script_writes_to(green.paths.rewrite_target_dir(&out_dir_var));
    md.push_block(&RUST, &green.base.image_inline);

    sys()
        .fs
        .create_dir_all(&out_dir_var)
        .map_err(|e| anyhow!("Failed to `mkdir -p {out_dir_var}`: {e}"))?;

    let run_stage = Stage::try_new(format!("run-{crate_id}"))?;
    let out_stage = Stage::output(mdid)?;

    let mut mds = green.paths.new_mds_cache(&target_path);

    let previous_md = mds.load(previous_mdid)?;
    trace!("previous_md = {previous_md:?}");

    let Some(code_stage) = previous_md.code_stage() else {
        bail!("BUG: no code stage found in {previous_md:?}")
    };

    let previous_out_stage = Stage::output(previous_mdid)?;
    let previous_out_dst = {
        let name = exe.file_name().expect("PROOF: exe has file_name");
        let name = name.replacen('-', "_", 2);
        let base = exe.parent().and_then(Utf8Path::file_name).expect("PROOF: exe has parent");
        format!("/{base}/_{name}-{previous_mdid}")
    };

    let mut run_block = format!("FROM {RST} AS {run_stage}\n");

    run_block.push_str(&format!("WORKDIR {}\n", green.paths.rewrite_target_dir(&out_dir_var)));
    // Cargo runs build scripts with $PWD set to $CARGO_MANIFEST_DIR, not the code's dir. (TEST= pyrefly)
    run_block.push_str(&format!("WORKDIR {}\n", green.paths.rewrite(pkg_manifest_dir)));

    let mount_flag = |name, src: Option<_>, dst: &Utf8Path, swappity| {
        let src = src.as_deref().map(|src| format!(",source={src}")).unwrap_or_default();
        let mount = if swappity { format!(",dst={dst}{src}") } else { format!("{src},dst={dst}") };
        format!("  --mount=from={name}{mount} \\\n")
    };

    let exe = green.paths.rewrite_target_dir(&exe);
    run_block.push_str("RUN \\\n");
    run_block.push_str(&format!(
        "  --mount=from={previous_out_stage},source={previous_out_dst},dst={exe} \\\n"
    ));
    let code_stage_name = code_stage.name().to_string();
    let mut mounted: HashSet<_> = [code_stage_name.clone()].into();
    for (src, dst, swappity) in code_stage.mounts() {
        run_block.push_str(&mount_flag(&code_stage_name, src, &dst, swappity));
    }

    let mut extern_mds = mds.load_all(previous_md.deps())?;
    extern_mds.push(previous_md);
    let mds = md.sort_deps(extern_mds)?;
    info!("sorted {} deps", mds.len());

    if green.buildscriptsources() {
        // Mounts build scripts' dependencies' sources so build scripts that
        // read a dependency's bundled files at execution time may find them
        // (eg. https://lib.rs/crates/protoc-bin-vendored ships `protoc` binaries).
        for dep in &mds {
            let Some(dep_code) = dep.code_stage() else { continue };
            let name = dep_code.name();
            let true = mounted.insert(name.to_string()) else { continue };
            for (src, dst, swappity) in dep_code.mounts() {
                run_block.push_str(&mount_flag(name, src, &dst, swappity));
            }
        }
    }

    md.call_block(
        (&run_stage, run_block),
        crate_name,
        &green.paths,
        &green.set_envs,
        vars,
        exe.as_str(),
        (&out_stage, Some(&out_dir_var)),
    )?;

    md.out_block(&out_stage, &run_stage, &green.paths, &out_dir_var);

    let containerfile = md.render(&green, &mds);
    let key = result_key(containerfile.as_str());

    if green.runner.is_none() {
        return md.reuse(&green, &out_stage, &key, &out_dir_var).await;
    }

    let (md_path, containerfile_path) = md.finalize(&containerfile, &target_path, pkg_name)?;

    md.do_build(&green, &md_path, &containerfile_path, &out_stage, &key, &out_dir_var).await?;

    Ok(Wrapped::Done)
}

/// Running a `build.rs`, which is the other half of the wrapper: by this point the
/// script has already been *built* (that is a normal crate compilation), and cargo is
/// invoking the shim [`exe_dance`] left in its place, which calls back into us.
#[cfg(test)]
mod pipeline {
    use std::sync::Arc;

    use snapbox::str;

    use super::{Green, Vars, exec_build_script};
    use crate::{
        base_image::BaseImage,
        build::Effects,
        containerfile::assert_containerfile_eq,
        dirs::Paths,
        r#final::Final,
        runner::Runner,
        sys::{
            Sys,
            fake::{FakeBuilds, FakeFs},
            install,
        },
    };

    /// The `build.rs` binary cargo built for us, and the one it is running now.
    const BUILT: &str = "2222222222222222";
    const RUN: &str = "4444444444444444";
    /// A dependency of the build script itself.
    const DEP: &str = "3333333333333333";

    const SRC: &str = "$CARGO_HOME/registry/src/index.crates.io";
    const MANIFEST: &str =
        "/home/u/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/proc-macro2-1.0.100";
    const EXE: &str = "/work/target/release/build/proc-macro2-2222222222222222/build-script-build";
    const OUT: &str = "/work/target/release/build/proc-macro2-4444444444444444/out";
    /// As `sha256::digest` returns it: bare hex, `add_step` prepends the algorithm.
    const NIL: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    /// The Md left by compiling `build.rs` into an executable.
    fn built_md() -> String {
        format!(
            r#"
stamp = 1
this = "{BUILT}"
buildrs = true
deps = ["{DEP}"]
writes = ["build-script-build-{BUILT}"]

[[stages]]

[stages.Script]
stage = "rust-base"
script = "FROM docker.io/library/rust:1.99.0-slim AS rust-base"

[[stages]]

[stages.Cratesio]
stage = "cratesio-proc-macro2-1.0.100"
extracted = "{SRC}/proc-macro2-1.0.100"
name = "proc-macro2"
name_dash_version = "proc-macro2-1.0.100"
hash = "{NIL}"

[[stages]]

[stages.Script]
stage = "out-{BUILT}"
script = """
FROM scratch AS out-{BUILT}
COPY --link --from=dep-x-proc-macro2-1.0.100-{BUILT} /target/release/build/proc-macro2-{BUILT} /proc-macro2-{BUILT}"""
"#
        )[1..]
            .to_owned()
    }

    /// A crate the build script links against, whose own source may need mounting.
    fn dep_md() -> String {
        format!(
            r#"
stamp = 1
this = "{DEP}"
writes = ["libunicode_ident-{DEP}.rlib"]

[[stages]]

[stages.Script]
stage = "rust-base"
script = "FROM docker.io/library/rust:1.99.0-slim AS rust-base"

[[stages]]

[stages.Cratesio]
stage = "cratesio-unicode-ident-1.0.14"
extracted = "{SRC}/unicode-ident-1.0.14"
name = "unicode-ident"
name_dash_version = "unicode-ident-1.0.14"
hash = "{NIL}"

[[stages]]

[stages.Script]
stage = "out-{DEP}"
script = """
FROM scratch AS out-{DEP}
COPY --link --from=dep-n-unicode-ident-1.0.14-{DEP} /target/release/deps /deps"""
"#
        )[1..]
            .to_owned()
    }

    /// `CARGO_CRATE_NAME` is unset while *running* a build script, which is how
    /// [`crate::wrap::call_config`] tells the two phases apart.
    fn vars() -> Vars {
        [
            ("CARGO_MANIFEST_DIR", MANIFEST),
            ("CARGO_PKG_NAME", "proc-macro2"),
            ("CARGO_PKG_VERSION", "1.0.100"),
            ("OUT_DIR", OUT),
            // Set by cargo for build scripts only: these must cross into the container.
            ("HOST", "x86_64-unknown-linux-gnu"),
            ("TARGET", "x86_64-unknown-linux-gnu"),
            ("OPT_LEVEL", "3"),
            ("PROFILE", "release"),
            ("NUM_JOBS", "32"),
            // Must not: host-specific, and would bust the cache.
            ("CARGO_HOME", "/home/u/.cargo"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
    }

    fn green(experiments: &[&str]) -> Green {
        Green {
            runner: Runner::Docker,
            base: BaseImage {
                image_inline: "FROM docker.io/library/rust:1.99.0-slim AS rust-base".to_owned(),
                ..Default::default()
            },
            r#final: Final { path: Some("/work/recipe.Dockerfile".into()) },
            experiment: ["finalpathnonprimary"]
                .into_iter()
                .chain(experiments.iter().copied())
                .map(ToOwned::to_owned)
                .collect(),
            paths: Paths {
                cargo_home: "/home/u/.cargo".into(),
                cwd: "/work".into(),
                host_target_dir: Some("/work/target".into()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Returns the generated Containerfile and the Md recorded for this run.
    fn run(experiments: &[&str]) -> (String, String) {
        let fs = Arc::new(FakeFs::default());
        fs.file(format!("/work/target/release/{BUILT}.toml"), built_md());
        fs.file(format!("/work/target/release/{DEP}.toml"), dep_md());
        let builds = Arc::new(FakeBuilds {
            effects: Effects {
                written: vec!["out/generated.rs".into()],
                // `cargo::rustc-env=` lines the script printed, for dependents to inherit.
                rustc_envs: [("PROC_MACRO2_SPAN".to_owned(), "1".to_owned())].into_iter().collect(),
                ..Effects::default()
            },
            ..FakeBuilds::default()
        });
        let _guard = install(Sys {
            fs: Arc::clone(&fs) as _,
            builds: Arc::clone(&builds) as _,
            ..Sys::fake()
        });

        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(exec_build_script(green(experiments), EXE.into(), &vars()))
            .unwrap();

        let containerfile = format!("/work/target/release/proc-macro2-{RUN}.Dockerfile");
        assert_eq!(builds.built(), [containerfile.as_str()]);
        (
            fs.read(&containerfile).unwrap(),
            fs.read(format!("/work/target/release/{RUN}.toml")).unwrap(),
        )
    }

    #[test]
    fn a_build_script_runs_against_its_own_crate_source() {
        assert_containerfile_eq!(
            run(&[]).0,
            str![[r#"
# syntax=docker.io/docker/dockerfile:1
# check=error=true
# Generated by https://github.com/fenollp/supergreen v0.27.0

FROM docker.io/library/rust:1.99.0-slim AS rust-base
ARG SOURCE_DATE_EPOCH=42


FROM scratch AS cratesio-unicode-ident-1.0.14
ADD --chmod=0664 --unpack --checksum=sha256:0000000000000000000000000000000000000000000000000000000000000000 \
  https://static.crates.io/crates/unicode-ident/unicode-ident-1.0.14.crate /
FROM scratch AS out-3333333333333333
COPY --link --from=dep-n-unicode-ident-1.0.14-3333333333333333 /target/release/deps /deps

FROM scratch AS cratesio-proc-macro2-1.0.100
ADD --chmod=0664 --unpack --checksum=sha256:0000000000000000000000000000000000000000000000000000000000000000 \
  https://static.crates.io/crates/proc-macro2/proc-macro2-1.0.100.crate /
FROM scratch AS out-2222222222222222
COPY --link --from=dep-x-proc-macro2-1.0.100-2222222222222222 /target/release/build/proc-macro2-2222222222222222 /proc-macro2-2222222222222222

FROM rust-base AS run-z-proc-macro2-1.0.100-4444444444444444
WORKDIR /target/release/build/proc-macro2-4444444444444444/out
WORKDIR $CARGO_HOME/registry/src/index.crates.io/proc-macro2-1.0.100
RUN \
  --mount=from=out-2222222222222222,source=/proc-macro2-2222222222222222/_build_script_build-2222222222222222,dst=/target/release/build/proc-macro2-2222222222222222/build-script-build \
  --mount=from=cratesio-proc-macro2-1.0.100,source=/proc-macro2-1.0.100,dst=$CARGO_HOME/registry/src/index.crates.io/proc-macro2-1.0.100 \
    env CARGO_MANIFEST_DIR=$CARGO_HOME/registry/src/index.crates.io/proc-macro2-1.0.100 \
        CARGO_PKG_NAME=proc-macro2 \
        CARGO_PKG_VERSION=1.0.100 \
        HOST=x86_64-unknown-linux-gnu \
        NUM_JOBS=1 \
        OPT_LEVEL=3 \
        OUT_DIR=/target/release/build/proc-macro2-4444444444444444/out \
        PROFILE=release \
        TARGET=x86_64-unknown-linux-gnu \
        CARGOGREEN=1 \
      /target/release/build/proc-macro2-2222222222222222/build-script-build \
        1>          /target/release/build/proc-macro2-4444444444444444/out-4444444444444444-stdout \
        2>          /target/release/build/proc-macro2-4444444444444444/out-4444444444444444-stderr \
        || echo $? >/target/release/build/proc-macro2-4444444444444444/out-4444444444444444-errcode\
  ; find /target/release/build/proc-macro2-4444444444444444/out/ /target/release/build/proc-macro2-4444444444444444/out-4444444444444444-* -exec touch --no-dereference --date=@$SOURCE_DATE_EPOCH '{}' + \
 || echo $? >/target/release/build/proc-macro2-4444444444444444/out-4444444444444444-errcode
FROM scratch AS out-4444444444444444
COPY --link --from=run-z-proc-macro2-1.0.100-4444444444444444 /target/release/build/proc-macro2-4444444444444444/out /out
COPY --link --from=run-z-proc-macro2-1.0.100-4444444444444444 /target/release/build/proc-macro2-4444444444444444/out-4444444444444444-* /

"#]]
        );
    }

    /// The Md tells dependents where this run wrote (`writes_to`) and which env vars
    /// the script asked them to compile with (`set_envs`).
    #[test]
    fn the_md_records_out_dir_and_rustc_envs() {
        assert_containerfile_eq!(
            run(&[]).1,
            str![[r#"
stamp = 1
this = "4444444444444444"
deps = [
    "3333333333333333",
    "2222222222222222",
]
buildrs = true
writes_to = "/target/release/build/proc-macro2-4444444444444444/out"
writes = ["out/generated.rs"]

[set_envs]
PROC_MACRO2_SPAN = "1"

[[stages]]

[stages.Script]
stage = "rust-base"
script = "FROM docker.io/library/rust:1.99.0-slim AS rust-base"

[[stages]]

[stages.Script]
stage = "run-z-proc-macro2-1.0.100-4444444444444444"
script = '''
FROM rust-base AS run-z-proc-macro2-1.0.100-4444444444444444
WORKDIR /target/release/build/proc-macro2-4444444444444444/out
WORKDIR $CARGO_HOME/registry/src/index.crates.io/proc-macro2-1.0.100
RUN \
  --mount=from=out-2222222222222222,source=/proc-macro2-2222222222222222/_build_script_build-2222222222222222,dst=/target/release/build/proc-macro2-2222222222222222/build-script-build \
  --mount=from=cratesio-proc-macro2-1.0.100,source=/proc-macro2-1.0.100,dst=$CARGO_HOME/registry/src/index.crates.io/proc-macro2-1.0.100 \
    env CARGO_MANIFEST_DIR=$CARGO_HOME/registry/src/index.crates.io/proc-macro2-1.0.100 \
        CARGO_PKG_NAME=proc-macro2 \
        CARGO_PKG_VERSION=1.0.100 \
        HOST=x86_64-unknown-linux-gnu \
        NUM_JOBS=1 \
        OPT_LEVEL=3 \
        OUT_DIR=/target/release/build/proc-macro2-4444444444444444/out \
        PROFILE=release \
        TARGET=x86_64-unknown-linux-gnu \
        CARGOGREEN=1 \
      /target/release/build/proc-macro2-2222222222222222/build-script-build \
        1>          /target/release/build/proc-macro2-4444444444444444/out-4444444444444444-stdout \
        2>          /target/release/build/proc-macro2-4444444444444444/out-4444444444444444-stderr \
        || echo $? >/target/release/build/proc-macro2-4444444444444444/out-4444444444444444-errcode\
  ; find /target/release/build/proc-macro2-4444444444444444/out/ /target/release/build/proc-macro2-4444444444444444/out-4444444444444444-* -exec touch --no-dereference --date=@$SOURCE_DATE_EPOCH '{}' + \
 || echo $? >/target/release/build/proc-macro2-4444444444444444/out-4444444444444444-errcode'''

[[stages]]

[stages.Script]
stage = "out-4444444444444444"
script = """
FROM scratch AS out-4444444444444444
COPY --link --from=run-z-proc-macro2-1.0.100-4444444444444444 /target/release/build/proc-macro2-4444444444444444/out /out
COPY --link --from=run-z-proc-macro2-1.0.100-4444444444444444 /target/release/build/proc-macro2-4444444444444444/out-4444444444444444-* /"""

"#]]
        );
    }

    /// With `buildscriptsources`, deps' sources are mounted too, for scripts that read
    /// files shipped inside a dependency's crate tarball.
    #[test]
    fn dependency_sources_are_mounted_under_the_experiment() {
        let plain = run(&[]).0;
        let with = run(&["buildscriptsources"]).0;
        assert!(!plain.contains("cratesio-unicode-ident-1.0.14,"), "in {plain}");
        assert!(with.contains("--mount=from=cratesio-unicode-ident-1.0.14,"), "in {with}");
    }
}
