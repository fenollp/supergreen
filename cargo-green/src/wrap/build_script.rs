use std::{
    env,
    fs::{self},
};

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use log::{error, info, trace};

use crate::{
    green::Green,
    logging::{self},
    md::{mds::Mds, Md, MdId},
    stage::{AsStage, Stage, RST, RUST},
    target_dir::virtual_target_dir,
    wrap::call_config,
    ENV, PKG, VSN,
};

#[macro_export]
macro_rules! ENV_EXECUTE_BUILDRS {
    () => {
        "CARGOGREEN_EXECUTE_BUILDRS_"
    };
}

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
pub(crate) fn exe_dance(mdid: MdId, crate_name: &str, out_dir: &Utf8Path) -> String {
    format!(
        r#"
  ; mv {out_dir}/{crate_name}-{mdid} {out_dir}/_{crate_name}-{mdid} \
 && printf '#!/bin/sh\nenv {var}=$0 {PKG}\n' >{out_dir}/{crate_name}-{mdid} \
 && chmod +x {out_dir}/{crate_name}-{mdid} \
"#,
        var = ENV_EXECUTE_BUILDRS!(),
    )[1..]
        .to_owned()
}

pub(crate) async fn exec(green: Green, exe: Utf8PathBuf) -> Result<()> {
    assert!(env::var_os(ENV!()).is_none(), "It's turtles all the way down!");
    env::set_var(ENV!(), "1");

    assert!(!green.runner.is_none(), "exec() called with Runner::None");

    let (crate_name, pkg_name, pkg_version, _) = call_config();

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
    let out_dir_var: Utf8PathBuf = env::var("OUT_DIR").expect("$OUT_DIR").into();
    let Some(mdid) = || -> Option<_> {
        // name: proc-macro2-b97492fdd0201a99
        let name = out_dir_var.parent()?.file_name()?;
        // mdid: b97492fdd0201a99
        let mdid: MdId = name.rsplit('-').next()?.into();
        Some(mdid)
    }() else {
        bail!("BUG: malformed $OUT_DIR {out_dir_var:?}")
    };

    // Z: for eggZecuting build scripts
    let full_pkg_id = format!("Z {pkg_name} {pkg_version}-{mdid}");
    logging::setup(&full_pkg_id);

    info!("{PKG}@{VSN} original args: {exe:?} green={green:?}");

    do_exec(
        green,
        crate_name.as_deref(),
        &pkg_name,
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
    crate_id: String,
    out_dir_var: Utf8PathBuf,
    exe: Utf8PathBuf,
    target_path: Utf8PathBuf,
    previous_mdid: MdId,
    mdid: MdId,
) -> Result<()> {
    let mut md: Md = mdid.into();
    md.build_script_writes_to(virtual_target_dir(&out_dir_var));
    md.push_block(&RUST, &green.base.image_inline);

    fs::create_dir_all(&out_dir_var)
        .map_err(|e| anyhow!("Failed to `mkdir -p {out_dir_var}`: {e}"))?;

    let run_stage = Stage::try_new(format!("run-{crate_id}"))?;
    let out_stage = Stage::output(mdid)?;

    let mut mds = Mds::new(&target_path);

    let previous_md = mds.load(previous_mdid)?;
    trace!("previous_md = {previous_md:?}");

    let Some(code_stage) = previous_md.code_stage() else {
        bail!("BUG: no code stage found in {previous_md:?}")
    };

    let previous_out_stage = Stage::output(previous_mdid)?;
    let previous_out_dst = {
        let name = exe.file_name().expect("PROOF: already ensured path has file_name");
        let name = name.replacen('-', "_", 2);
        format!("/_{name}-{previous_mdid}")
    };

    let mut run_block = format!("FROM {RST} AS {run_stage}\n");

    run_block.push_str(&format!("WORKDIR {}\n", virtual_target_dir(&out_dir_var)));
    let mut code_stage_mounts = code_stage.mounts();
    let Some((_, code_dst, _)) = code_stage_mounts.pop() else {
        bail!("BUG: a crate should only have one build script")
    };
    assert_eq!(code_stage_mounts, vec![]);
    let code_dst = virtual_target_dir(&code_dst);
    run_block.push_str(&format!("WORKDIR {code_dst}\n"));

    run_block.push_str("RUN \\\n");
    run_block.push_str(&format!(
        "  --mount=from={previous_out_stage},source={previous_out_dst},dst={exe} \\\n",
        exe = virtual_target_dir(&exe)
    ));
    for (src, dst, swappity) in code_stage.mounts() {
        let name = code_stage.name();
        let src = src.as_deref().map(|src| format!(",source={src}")).unwrap_or_default();
        let mount = if swappity { format!(",dst={dst}{src}") } else { format!("{src},dst={dst}") };
        run_block.push_str(&format!("  --mount=from={name}{mount} \\\n"));
    }

    let mut extern_mds = mds.load_all(previous_md.deps())?;
    extern_mds.push(previous_md);
    let mds = md.sort_deps(extern_mds)?;
    info!("sorted {} deps", mds.len());

    md.call_block(
        (&run_stage, run_block),
        crate_name,
        &green.cargo_home,
        &green.set_envs,
        virtual_target_dir(&exe).as_str(),
        (&out_stage, Some(&out_dir_var)),
    )?;

    md.out_block(&out_stage, &run_stage, &out_dir_var, true);

    let (md_path, containerfile_path) = md.finalize(&green, &target_path, pkg_name, &mds)?;

    md.do_build(&green, &md_path, &containerfile_path, &out_stage, &out_dir_var).await
}
