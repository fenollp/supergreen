use std::{
    collections::HashSet,
    env,
    fs::{self},
};

use anyhow::{Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use log::{error, info, trace, warn};

use crate::{
    PKG, VSN,
    all_our_envs::OUT_DIR,
    cratesio::{self},
    dirs::Paths,
    green::Green,
    logging::{self},
    md::{Md, MdId},
    stage::{AsStage, RST, RUST, Stage},
    wrap::call_config,
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

pub(crate) async fn exec_build_script(green: Green, exe: Utf8PathBuf) -> Result<()> {
    let (crate_name, pkg_name, pkg_version, pkg_manifest_dir) = call_config();

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
    let out_dir_var: Utf8PathBuf = env::var(OUT_DIR!()).expect(OUT_DIR).into();
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

    if green.runner.is_none() {
        if green.paths.reuse_out(&Stage::output(mdid)?, &out_dir_var).await? {
            return Ok(());
        }
        todo!("fallback()");
    }

    do_exec(
        green,
        crate_name.as_deref(),
        &pkg_name,
        &pkg_manifest_dir,
        full_pkg_id.replace(' ', "-"),
        out_dir_var,
        exe,
        target_path,
        previous_mdid,
        mdid,
    )
    .await
    .inspect_err(|e| error!("Error: {e}"))
}

#[expect(clippy::too_many_arguments)]
async fn do_exec(
    green: Green,
    crate_name: Option<&str>,
    pkg_name: &str,
    pkg_manifest_dir: &Utf8Path,
    crate_id: String,
    out_dir_var: Utf8PathBuf,
    exe: Utf8PathBuf,
    target_path: Utf8PathBuf,
    previous_mdid: MdId,
    mdid: MdId,
) -> Result<()> {
    let mut md: Md = mdid.into();
    md.build_script_writes_to(green.paths.rewrite_target_dir(&out_dir_var));
    md.push_block(&RUST, &green.base.image_inline);

    fs::create_dir_all(&out_dir_var)
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

    fn mount_flag(name: &str, src: Option<Utf8PathBuf>, dst: &Utf8Path, swappity: bool) -> String {
        let src = src.as_deref().map(|src| format!(",source={src}")).unwrap_or_default();
        let mount = if swappity { format!(",dst={dst}{src}") } else { format!("{src},dst={dst}") };
        format!("  --mount=from={name}{mount} \\\n")
    }

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

    // Build scripts of crates that depend on a `links = ".."` crate receive $DEP_<links>_<key>
    // vars, set from that crate's own build script's metadata prints. Values are free-form and
    // may embed paths into that build script's $OUT_DIR (eg. tree-sitter's build script passes
    // "-I$DEP_WASMTIME_C_API_INCLUDE" to cc, pointing inside wasmtime-c-api-impl's buildrs out
    // dir) or into that crate's sources (eg. cxx's build script exports
    // $DEP_CXXBRIDGE1_HEADER=<cxx's crate dir>/include/cxx.h that cxx-build, run by dependents'
    // build scripts, symlinks under their own $OUT_DIR): mount these so such paths resolve.
    // TODO: also handle paths into git checkouts (see checkouts::as_stage)
    for (var, val) in env::vars().filter(|(var, _)| var.starts_with("DEP_")) {
        for dep_mdid in buildrs_out_mdids(&val, green.paths.target_dir().as_str()) {
            if [mdid, previous_mdid].contains(&dep_mdid) {
                continue;
            }
            let dep_md = match mds.load(dep_mdid) {
                Ok(dep_md) => dep_md,
                Err(e) => {
                    warn!("skipping ${var} buildrs out dir mount ({dep_mdid}): {e}");
                    continue;
                }
            };
            let Some(mount) = dep_md.buildrs_out_mount() else { continue };
            let true = mounted.insert(mount.name.to_string()) else { continue };
            let dep_mds = match mds.load_all(dep_md.deps()) {
                Ok(dep_mds) => dep_mds,
                Err(e) => {
                    warn!("skipping ${var} buildrs out dir mount ({dep_mdid}): {e}");
                    continue;
                }
            };
            info!("mounting ${var} buildrs out dir {}", mount.mount);
            let base = mount.mount.file_name().expect("PROOF: OUT_DIR mounts end in /out");
            run_block.push_str(&mount_flag(
                &mount.name,
                Some(format!("/{base}").into()),
                &mount.mount,
                false,
            ));
            extern_mds.extend(dep_mds);
            extern_mds.push(dep_md);
            md.mounts.insert(mount);
        }
        for (name, manifest_dir) in green.paths.cratesio_manifest_dirs(&val) {
            let Some(name_dash_version) = manifest_dir.file_name() else { continue };
            let Ok(stage) = Stage::cratesio(name_dash_version) else { continue };
            let true = mounted.insert(stage.to_string()) else { continue };
            let dep_code = match cratesio::named_stage(&green.paths, &name, &manifest_dir).await {
                Ok(dep_code) => dep_code,
                Err(e) => {
                    warn!("skipping ${var} sources mount ({manifest_dir}): {e}");
                    continue;
                }
            };
            info!("mounting ${var} sources {manifest_dir}");
            for (src, dst, swappity) in dep_code.mounts() {
                run_block.push_str(&mount_flag(dep_code.name(), src, &dst, swappity));
            }
            md.push_stage(&dep_code);
        }
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
                run_block.push_str(&mount_flag(name, src, &dst, swappity));
            }
        }
    }

    md.call_block(
        (&run_stage, run_block),
        crate_name,
        &green.paths,
        &green.set_envs,
        exe.as_str(),
        (&out_stage, Some(&out_dir_var)),
    )?;

    md.out_block(&out_stage, &run_stage, &green.paths, &out_dir_var);

    let (md_path, containerfile_path) = md.finalize(&green, &target_path, pkg_name, &mds)?;

    md.do_build(&green, &md_path, &containerfile_path, &out_stage, &out_dir_var).await
}

/// Paths in a $DEP_<links>_<key> value pointing into build scripts' $OUT_DIRs, as their MdIds.
///
/// Values are free-form: a path may sit mid-string (eg. within "-I/.." cflags) or come in
/// multiples, so substring-scan for target dir occurrences.
fn buildrs_out_mdids(val: &str, target_dir: &str) -> Vec<MdId> {
    let mut mdids = vec![];
    for (pos, _) in val.match_indices(target_dir) {
        let rest = &val[pos + target_dir.len()..];
        // eg. release/build/wasmtime-c-api-impl-1e0b13039f0dbd50/out/include
        let Some(pos) = rest.find("/build/") else { continue };
        let rest = &rest[pos + "/build/".len()..];
        let Some((dir, rest)) = rest.split_once('/') else { continue };
        let Some(after) = rest.strip_prefix("out") else { continue };
        if after.chars().next().is_some_and(|c| c != '/' && !c.is_whitespace()) {
            continue;
        }
        let Some(mdid) = dir.rsplit('-').next() else { continue };
        if mdid.len() == 16 && mdid.bytes().all(|b| b.is_ascii_hexdigit()) {
            mdids.push(mdid.into());
        }
    }
    mdids
}

#[test]
fn dep_var_buildrs_out_mdids() {
    let td = "/tmp/clis-x";

    let val = "/tmp/clis-x/release/build/wasmtime-c-api-impl-1e0b13039f0dbd50/out/include";
    assert_eq!(buildrs_out_mdids(val, td), vec!["1e0b13039f0dbd50".into()]);

    // Flag-embedded, multiple paths, one ending right at /out
    let val = "-I/tmp/clis-x/release/build/zstd-sys-0123456789abcdef/out \
               -DX=/tmp/clis-x/armv7-unknown-linux-musleabihf/release/build/lz4-sys-fedcba9876543210/out";
    assert_eq!(
        buildrs_out_mdids(val, td),
        vec!["0123456789abcdef".into(), "fedcba9876543210".into()]
    );

    assert_eq!(buildrs_out_mdids("/tmp/clis-x/release/build/foo-0123456789abcdef/output", td), []);
    assert_eq!(buildrs_out_mdids("/tmp/clis-x/release/build/foo-badbeef/out", td), []);
    assert_eq!(buildrs_out_mdids("/tmp/clis-x/release/deps/libfoo-0123456789abcdef.rlib", td), []);
    assert_eq!(buildrs_out_mdids("TRUE", td), []);
}

impl Paths {
    /// Paths in a $DEP_<links>_<key> value pointing into crates.io crates' sources,
    /// as (crate name, host manifest dir).
    fn cratesio_manifest_dirs(&self, val: &str) -> Vec<(String, Utf8PathBuf)> {
        let mut found = vec![];
        let home = self.cratesio_home();
        let prefix = format!("{home}/");
        for (pos, _) in val.match_indices(prefix.as_str()) {
            let rest = &val[pos + prefix.len()..];
            // eg. index.crates.io-1949cf8c6b5b557f/cxx-1.0.197/include/cxx.h
            let mut segments = rest.split('/');
            let (Some(index), Some(name_dash_version)) = (segments.next(), segments.next()) else {
                continue;
            };
            // A path ending at the crate dir may carry non-path trailers (eg. quotes, cflags)
            let name_dash_version =
                name_dash_version.split(|c: char| c.is_whitespace() || "\"':,".contains(c)).next();
            let Some(name_dash_version) = name_dash_version.filter(|ndv| !ndv.is_empty()) else {
                continue;
            };
            // "<name>-<version>": version starts with a digit but crate names may embed digit-led
            // segments too (eg. utf-8-0.7.6), so split at the last dash followed by a digit.
            let Some(name) = name_dash_version
                .match_indices('-')
                .rfind(|&(i, _)| {
                    name_dash_version
                        .as_bytes()
                        .get(i + 1)
                        .copied()
                        .is_some_and(|b| b.is_ascii_digit())
                })
                .map(|(i, _)| &name_dash_version[..i])
            else {
                continue;
            };
            found.push((name.to_owned(), home.join(index).join(name_dash_version)));
        }
        found
    }
}

#[test]
fn dep_var_cratesio_manifest_dirs() {
    let paths = Paths { cargo_home: "/home/user/.cargo".into(), ..Default::default() };
    let srcs = "/home/user/.cargo/registry/src";

    let val = format!("{srcs}/index.crates.io-1949cf8c6b5b557f/cxx-1.0.197/include/cxx.h");
    assert_eq!(
        paths.cratesio_manifest_dirs(&val),
        vec![(
            "cxx".to_owned(),
            format!("{srcs}/index.crates.io-1949cf8c6b5b557f/cxx-1.0.197").into()
        )]
    );

    // Digit-led name segment + path ending right at the crate dir
    let val = format!("{srcs}/index.crates.io-1949cf8c6b5b557f/utf-8-0.7.6 -DX");
    assert_eq!(
        paths.cratesio_manifest_dirs(&val),
        vec![(
            "utf-8".to_owned(),
            format!("{srcs}/index.crates.io-1949cf8c6b5b557f/utf-8-0.7.6").into()
        )]
    );

    let val = "/home/user/.cargo/registry/cache/index.crates.io-1949cf8c6b5b557f/cxx-1.0.197.crate";
    assert_eq!(paths.cratesio_manifest_dirs(val), []);
    assert_eq!(paths.cratesio_manifest_dirs("TRUE"), []);
}
