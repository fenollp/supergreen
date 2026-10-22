use std::collections::HashSet;

use anyhow::{Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexSet;
use log::{info, trace, warn};

use crate::{
    PKG, VSN,
    all_our_envs::OUT_DIR,
    cratesio::{self},
    green::Green,
    md::{Md, MdId, NamedMount},
    stage::{AsStage, RST, RUST, Stage},
    sys::fs,
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
        "
mv {exe} {out_dir}/_{crate_name}-{mdid}
printf '#!/bin/sh\\nenv {var}=$0 {PKG}\\n' >{exe}
chmod +x {exe}
",
        var = CARGOGREEN_EXECUTEBUILDSCRIPT!(),
        exe = format!("{out_dir}/{crate_name}-{mdid}"),
    )[1..]
        .to_owned()
}

pub(crate) async fn exec_build_script(green: Green, exe: Utf8PathBuf) -> Result<()> {
    let (crate_name, pkg_name, pkg_version, pkg_manifest_dir) = green.call_config();

    // exe: /target/release/build/proc-macro2-2f938e044e3f79bf/build-script-build
    let Some((previous_mdid, target_path)) = || -> Option<_> {
        let mdid = MdId::from_buildrs_exe(&exe)?;
        // target_path: /target/release
        let target_path = exe.ancestors().nth(3)?.to_owned();
        Some((mdid, target_path))
    }() else {
        bail!("BUG: malformed buildrs exe {exe:?}")
    };

    // $OUT_DIR: /target/release/build/proc-macro2-b97492fdd0201a99/out
    let Some(out_dir_var) = green.out_dir() else { bail!("BUG: unset {OUT_DIR}") };
    let Some(mdid) = MdId::from_out_dir_var(out_dir_var) else {
        bail!("BUG: malformed {OUT_DIR}={out_dir_var:?}")
    };

    // Z: for eggZecuting build scripts
    let full_pkg_id = format!("Z {pkg_name} {pkg_version}-{mdid}");
    green.setup_logging(&full_pkg_id)?;

    info!("{PKG}@{VSN} original args: {exe:?} green={green:?}");

    if green.runner.is_none() {
        if green.paths.reuse_out(&Stage::output(mdid), out_dir_var).await? {
            return Ok(());
        }
        todo!("fallback()");
    }

    let out_dir_var = out_dir_var.to_owned();
    do_exec(
        green,
        crate_name.as_deref(),
        &pkg_name,
        &pkg_manifest_dir,
        full_pkg_id.replace(' ', "-"),
        &out_dir_var,
        exe,
        target_path,
        previous_mdid,
        mdid,
    )
    .await
}

#[expect(clippy::too_many_arguments)]
async fn do_exec(
    green: Green,
    crate_name: Option<&str>,
    pkg_name: &str,
    pkg_manifest_dir: &Utf8Path,
    crate_id: String,
    out_dir_var: &Utf8Path,
    exe: Utf8PathBuf,
    target_path: Utf8PathBuf,
    previous_mdid: MdId,
    mdid: MdId,
) -> Result<()> {
    let mut md: Md = mdid.into();
    md.build_script_writes_to(green.paths.rewrite_target_dir(out_dir_var));
    md.push_block(&RUST, &green.base.image_inline);

    fs().create_dir_all(out_dir_var)
        .map_err(|e| anyhow!("Failed to `mkdir -p {out_dir_var}`: {e}"))?;

    let run_stage = Stage::try_new(format!("run-{crate_id}"))?;
    let out_stage = Stage::output(mdid);

    let mut mds = green.paths.new_mds_cache(&target_path);

    let previous_md = mds.load(previous_mdid)?;
    trace!("previous_md = {previous_md:?}");

    let Some(code_stage) = previous_md.code_stage() else {
        bail!("BUG: no code stage found in {previous_md:?}")
    };

    let previous_out_stage = Stage::output(previous_mdid);
    let previous_out_dst = {
        let name = exe.file_name().expect("PROOF: exe has file_name");
        let name = name.replacen('-', "_", 2);
        let base = exe.parent().and_then(Utf8Path::file_name).expect("PROOF: exe has parent");
        format!("/{base}/_{name}-{previous_mdid}")
    };

    let mut run_block = format!("FROM {RST} AS {run_stage}\n");

    run_block.push_str(&format!("WORKDIR {}\n", green.paths.rewrite_target_dir(out_dir_var)));
    // Cargo runs build scripts with $PWD set to $CARGO_MANIFEST_DIR, not the code's dir. (TEST= pyrefly)
    run_block.push_str(&format!("WORKDIR {}\n", green.paths.rewrite(pkg_manifest_dir)));

    let mount_flag = |name: &str, src: Option<_>, dst: &Utf8Path, swappity| {
        let dst = green.paths.rewrite(dst);
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
    let mut dsts = HashSet::new();
    for (src, dst, swappity) in code_stage.mounts() {
        run_block.push_str(&mount_flag(&code_stage_name, src, &dst, swappity));
        dsts.insert(dst);
    }

    let mut extern_mds = mds.load_all(previous_md.deps())?;
    extern_mds.push(previous_md);

    // A crate can declare `links = "foo"` in its Cargo.toml.
    // Its build script can then print `cargo:KEY=VALUE` lines.
    // Cargo turns these into `$DEP_<LINKS>_<KEY> --> VALUE` environment variables,
    // but only for the build scripts of crates that depend directly on it.
    // This is how `-sys` crates tell their dependents where things are.
    // For example, libssh2-sys builds libssh2 into its $OUT_DIR and exports `DEP_SSH2_ROOT=<its $OUT_DIR>`
    // and `DEP_SSH2_INCLUDE=<its $OUT_DIR>/include`.
    // The dependent's build script (here ssh2's) then reads those paths to find headers or libraries.
    //
    // So before running a build script, let's search through `DEP_*` values for paths to some build script's $OUT_DIR (under the target dir).
    // Each match gives the MdId of the build-script run that created that dir, then loads that run's md.toml.
    // If the run actually wrote output, it adds a mount of to run's `out-<mdid>` stage at the same $OUT_DIR path.
    for links_mdid in green.out_dirs_in_dep_vars() {
        let Ok(links_md) = mds.load(links_mdid) else { continue };
        let Some(mount) = links_md.buildrs_out_mount() else { continue };
        info!("mounting $DEP_* -links'd buildrs out dir from {}: {}", mount.name, mount.mount);
        md.insert_buildrs_out_dir_mount(links_mdid, mount);
    }
    mds.extend_loading_depth1(&mut extern_mds, md.buildrs_runs())?;
    for NamedMount { name, mount } in md.out_dir_mounts() {
        run_block.push_str(&format!("  --mount=from={name},source=/out,dst={mount} \\\n"));
    }

    // $DEP_<LINKS>_<KEY> values may also point into crates' sources: eg. cxx's build script exports
    // $DEP_CXXBRIDGE1_HEADER=<cxx's crate dir>/include/cxx.h that cxx-build, run by dependents'
    // build scripts, symlinks under their own $OUT_DIR. Mount these so such paths resolve.
    // TODO: also handle paths into git checkouts (see checkouts::as_stage)
    for (name, manifest_dir) in green.cratesio_dirs_in_dep_vars() {
        let name_dash_version = manifest_dir.file_name().expect("PROOF: <name>-<version> dir");
        let Ok(stage) = Stage::cratesio(name_dash_version) else { continue };
        let true = mounted.insert(stage.to_string()) else { continue };
        let dep_code = match cratesio::named_stage(&green.paths, &name, &manifest_dir).await {
            Ok(dep_code) => dep_code,
            Err(e) => {
                warn!("skipping $DEP_* -pointed sources mount ({manifest_dir}): {e}");
                continue;
            }
        };
        info!("mounting $DEP_* -pointed sources {manifest_dir}");
        for (src, dst, swappity) in dep_code.mounts() {
            run_block.push_str(&mount_flag(dep_code.name(), src, &dst, swappity));
        }
        md.push_stage(&dep_code);
    }

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
                let false = dsts.contains(&dst) else { continue };
                run_block.push_str(&mount_flag(name, src, &dst, swappity));
                dsts.insert(dst);
            }
        }
    }

    md.call_block(
        (&run_stage, run_block),
        crate_name,
        &green.paths,
        false,
        &green.set_envs,
        &green.env,
        exe.as_str(),
        Some(out_dir_var),
    )?;

    md.out_block(&out_stage, &run_stage, &green.paths, out_dir_var);

    let (md_path, containerfile_path) = md.finalize(&green, &target_path, pkg_name, &mds)?;

    md.do_build(&green, &md_path, &containerfile_path, &out_stage, out_dir_var).await
}

impl Green {
    /// Build script runs whose `$OUT_DIR` is pointed into by `$DEP_<LINKS>_<KEY>` values.
    fn out_dirs_in_dep_vars(&self) -> IndexSet<MdId> {
        let target_dir = format!("{}/", self.paths.target_dir());
        self.dep_vars_parts()
            .filter_map(|part| part.split_once(&target_dir))
            .filter_map(|(_, path)| MdId::from_out_dir_var(path.into()))
            .collect()
    }

    /// crates.io crates whose sources `$DEP_<LINKS>_<KEY>` values point into,
    /// as (crate name, host manifest dir).
    fn cratesio_dirs_in_dep_vars(&self) -> IndexSet<(String, Utf8PathBuf)> {
        let home = self.paths.cratesio_home();
        let prefix = format!("{home}/");
        self.dep_vars_parts()
            .filter_map(|part| part.split_once(&prefix))
            .filter_map(|(_, path)| {
                // eg. index.crates.io-1949cf8c6b5b557f/cxx-1.0.197/include/cxx.h
                let mut dirs = path.split('/');
                let (index, name_dash_version) = (dirs.next()?, dirs.next()?);
                let name = cratesio::name_of(name_dash_version)?;
                Some((name.to_owned(), home.join(index).join(name_dash_version)))
            })
            .collect()
    }

    /// Words of `$DEP_<LINKS>_<KEY>` values, which are free-form:
    /// paths may come in lists, quoted or embedded in flags (eg. `-I/..`).
    fn dep_vars_parts(&self) -> impl Iterator<Item = &str> {
        self.env
            .iter()
            .filter(|(var, _)| var.starts_with("DEP_"))
            .flat_map(|(_, val)| val.split([':', ' ', ',', ';', '"', '\'']))
    }
}

#[test]
fn out_dirs_in_dep_vars() {
    let mut green = Green::default();
    green.paths.host_target_dir = Some("/t".into());
    green.env = [
        ("DEP_SSH2_ROOT", "/t/debug/build/libssh2-sys-9999999999999999/out"),
        ("DEP_Z_INCLUDE", "/t/armv7-unknown-linux-musleabihf/debug/build/z-1111111111111111/out/include:/x"),
        ("DEP_ZSTD_CFLAGS", "-I/t/release/build/zstd-sys-0123456789abcdef/out -DX=/t/release/build/lz4-sys-fedcba9876543210/out"),
        ("DEP_SSH2_AGAIN", "/t/debug/build/libssh2-sys-9999999999999999/out/include"),
        ("DEP_A_OUTPUT", "/t/release/build/foo-0123456789abcdef/output"),
        ("DEP_A_RLIB", "/t/release/deps/libfoo-0123456789abcdef.rlib"),
        ("DEP_A_FLAG", "TRUE"),
        ("DEP_W_ROOT", "/elsewhere/debug/build/w-3333333333333333/out"),
        ("DEP_T_ROOT", "/tt/debug/build/w-3333333333333333/out"),
        ("OUT_DIR", "/t/debug/build/y-2222222222222222/out"),
    ]
    .into_iter()
    .map(|(var, val)| (var.to_owned(), val.to_owned()))
    .collect();
    assert_eq!(
        green.out_dirs_in_dep_vars(),
        [
            "9999999999999999".into(),
            "1111111111111111".into(),
            "0123456789abcdef".into(),
            "fedcba9876543210".into(),
        ]
        .into()
    );
}

#[test]
fn cratesio_dirs_in_dep_vars() {
    let mut green = Green::default();
    green.paths.cargo_home = "/c".into();
    let index = "/c/registry/src/index.crates.io-1949cf8c6b5b557f";
    green.env = [
        ("DEP_CXXBRIDGE1_HEADER", format!("{index}/cxx-1.0.197/include/cxx.h")),
        ("DEP_U_FLAGS", format!("-I{index}/utf-8-0.7.6 -DX")),
        ("DEP_B_QUOTED", format!("'{index}/bzip2-sys-0.1.11+1.0.8'")),
        ("DEP_CXX_AGAIN", format!("{index}/cxx-1.0.197/include")),
        (
            "DEP_A_CACHED",
            "/c/registry/cache/index.crates.io-1949cf8c6b5b557f/cxx-1.0.197.crate".to_owned(),
        ),
        ("DEP_A_FLAG", "TRUE".to_owned()),
        ("CARGO_MANIFEST_DIR", format!("{index}/y-1.0.0")),
    ]
    .into_iter()
    .map(|(var, val)| (var.to_owned(), val))
    .collect();
    assert_eq!(
        green.cratesio_dirs_in_dep_vars(),
        [
            ("cxx".to_owned(), format!("{index}/cxx-1.0.197").into()),
            ("utf-8".to_owned(), format!("{index}/utf-8-0.7.6").into()),
            ("bzip2-sys".to_owned(), format!("{index}/bzip2-sys-0.1.11+1.0.8").into()),
        ]
        .into()
    );
}
