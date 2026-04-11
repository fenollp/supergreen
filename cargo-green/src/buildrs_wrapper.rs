use std::{
    env,
    fs::{self},
};

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use log::{error, info, trace};
use tokio::process::Command;

use crate::{
    build::SHELL,
    ext::CommandExt,
    green::Green,
    logging::{self},
    md::{Md, MdId, Mds},
    stage::{AsStage, Stage, RST, RUST},
    target_dir::virtual_target_dir,
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

pub(crate) fn call_config() -> (Option<String>, String, String, Utf8PathBuf) {
    (
        env::var("CARGO_CRATE_NAME").ok(), // Unset when executing buildrs (always set when building)
        env::var("CARGO_PKG_NAME").expect("$CARGO_PKG_NAME"),
        env::var("CARGO_PKG_VERSION").expect("$CARGO_PKG_VERSION"),
        env::var("CARGO_MANIFEST_DIR").expect("$CARGO_MANIFEST_DIR").into(),
    )
}

pub(crate) async fn exec_buildrs(green: Green, exe: Utf8PathBuf) -> Result<()> {
    assert!(env::var_os(ENV!()).is_none(), "It's turtles all the way down!");
    env::set_var(ENV!(), "1");

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

    do_exec_buildrs(
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
async fn do_exec_buildrs(
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
    md.buildrs = true;
    md.writes_to = virtual_target_dir(&out_dir_var);
    md.push_block(&RUST, green.base.image_inline.clone().unwrap());

    fs::create_dir_all(&out_dir_var)
        .map_err(|e| anyhow!("Failed to `mkdir -p {out_dir_var}`: {e}"))?;

    let run_stage = Stage::try_new(format!("run-{crate_id}"))?;
    // let out_stage = Stage::try_new(format!("ran-{mdid}"))?;
    let out_stage = Stage::output(mdid)?;

    let mut mds = Mds::default(); //FIXME: unpub?

    let previous_md_path = previous_mdid.path(&target_path);
    let previous_md = mds.get_or_read(&previous_md_path)?;
    trace!("previous_md_path = {previous_md_path}");
    trace!("previous_md      = {previous_md:?}");

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
    run_block.push_str(&format!("SHELL {SHELL:?}\n"));
    run_block.push_str(&format!("WORKDIR {}\n", virtual_target_dir(&out_dir_var)));
    for (_, code_dst, _) in code_stage.mounts() {
        let code_dst = virtual_target_dir(&code_dst);
        run_block.push_str(&format!("WORKDIR {code_dst}\n"));
    }
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

    // let target_path = previous_md_path.parent().unwrap();
    // let (mounts, mut mds) =
    //     assemble_build_dependencies(&mut md, "bin", "dep-info,link", [].into(), target_path)?;
    // mds.push(previous_md);
    // for NamedMount { name, src, dst } in mounts {
    //     run_block.push_str(&format!("  --mount=from={name},dst={dst},source={src} \\\n"));
    // }

    let mut extern_mds_and_paths = previous_md
        .deps()
        .iter()
        .map(|xtern| {
            let xtern_md_path = xtern.path(&target_path);
            let xtern_md = mds.get_or_read(&xtern_md_path)?;
            Ok((xtern_md_path, xtern_md))
        })
        .collect::<Result<Vec<_>>>()?;
    extern_mds_and_paths.push((previous_md_path, previous_md));
    let extern_md_paths = md.sort_deps(extern_mds_and_paths)?;
    info!("extern_md_paths: {}", extern_md_paths.len());

    let mds = extern_md_paths
        .into_iter()
        .map(|extern_md_path| mds.get_or_read(&extern_md_path))
        .collect::<Result<Vec<_>>>()?;

    md.run_block(
        (&run_stage, run_block),
        crate_name,
        &green.cargo_home,
        &green.set_envs,
        virtual_target_dir(&exe).to_string(),
        (&out_stage, &out_dir_var),
    )?;

    md.out_block(&out_stage, &run_stage, &out_dir_var, true);

    let containerfile_path = md.finalize(&green, &target_path, pkg_name, &mds)?;

    let fallback = async move {
        let mut cmd = Command::new(&exe);
        let cmd = cmd.kill_on_drop(true);
        let cmd = cmd.env_remove(ENV_EXECUTE_BUILDRS!());
        let status = cmd
            .spawn()
            .map_err(|e| anyhow!("Failed to spawn {}: {e}", cmd.show()))?
            .wait()
            .await
            .map_err(|e| anyhow!("Failed to wait {}: {e}", cmd.show()))?;
        if !status.success() {
            bail!("Failed in execute_buildrs")
        }
        Ok(())
    };

    md.do_build(&green, fallback, &containerfile_path, &out_stage, &out_dir_var, &target_path)
        .await?;

    Ok(())
}
