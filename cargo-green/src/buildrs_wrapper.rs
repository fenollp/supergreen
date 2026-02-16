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

pub(crate) fn rewrite_main(mdid: MdId, input: &Utf8Path) -> String {
    format!(
        r#"    {{ \
        cat {input} | sed -E 's/^(pub[()a-z]* +)?(async +)?fn +main/\1\2fn actual_{mdid}_main/' >/_ && mv /_ {input} ; \
        {{ \
          echo ; \
          echo 'fn main() {{' ; \
          echo '    use std::env::{{args_os, var_os}};' ; \
          echo '    if var_os("{var}").is_none() {{' ; \
          echo '        use std::process::{{Command, Stdio}};' ; \
          echo '        let mut cmd = Command::new("{PKG}");' ; \
          echo '        cmd.stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit());' ; \
          echo '        cmd.env("{var}", args_os().next().expect("{PKG}: getting buildrs arg0"));' ; \
          echo '        let res = cmd.spawn().expect("{PKG}: spawning buildrs").wait().expect("{PKG}: running builds");' ; \
          echo '        assert!(res.success());' ; \
          echo '    }} else {{' ; \
          echo '        actual_{mdid}_main();' ; \
          echo '    }}' ; \
          echo '}}' ; \
        }} >>{input} ; \
    }} && \
"#,
        var = ENV_EXECUTE_BUILDRS!(),
    )
}

pub(crate) async fn exec_buildrs(green: Green, exe: Utf8PathBuf) -> Result<()> {
    assert!(env::var_os(ENV!()).is_none(), "It's turtles all the way down!");
    env::set_var(ENV!(), "1");

    let krate_name = env::var("CARGO_PKG_NAME").expect("$CARGO_PKG_NAME");
    let krate_version = env::var("CARGO_PKG_VERSION").expect("$CARGO_PKG_VERSION");

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

    // Z: for executing build scripts
    let full_krate_id = format!("Z {krate_name} {krate_version}-{mdid}");
    logging::setup(&full_krate_id);

    info!("{PKG}@{VSN} original args: {exe:?} green={green:?}");

    do_exec_buildrs(
        green,
        &krate_name,
        // krate_version,
        full_krate_id.replace(' ', "-"),
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
    krate_name: &str,
    // krate_version: String,
    crate_id: String,
    out_dir_var: Utf8PathBuf,
    exe: Utf8PathBuf,
    target_path: Utf8PathBuf,
    previous_mdid: MdId,
    mdid: MdId,
) -> Result<()> {
    let mut md: Md = mdid.into();
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
        format!("/{name}-{previous_mdid}")
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

    let mut extern_mds_and_paths: Vec<_> = previous_md
        .deps()
        .iter()
        .map(|xtern| -> Result<_> {
            let xtern_md_path = xtern.path(&target_path);
            let xtern_md = mds.get_or_read(&xtern_md_path)?;
            Ok((xtern_md_path, xtern_md))
        })
        .collect::<Result<_>>()?;
    // md.short_externs.push(previous_md as short xtern) FIXME?? MAY counter assemble+outdirvar
    extern_mds_and_paths.push((previous_md_path, previous_md));
    let extern_md_paths = md.sort_deps(extern_mds_and_paths)?;
    info!("extern_md_paths: {}", extern_md_paths.len());

    let mds = extern_md_paths
        .into_iter()
        .map(|extern_md_path| mds.get_or_read(&extern_md_path))
        .collect::<Result<Vec<_>>>()?;

    md.run_block(
        &run_stage,
        &out_stage,
        &out_dir_var,
        format!("{env}= {exe}", env = ENV_EXECUTE_BUILDRS!(), exe = virtual_target_dir(&exe)),
        &green.set_envs,
        true, //FIXME: try "false" => Noneify?
        run_block,
    )?;

    md.out_block(&out_stage, &run_stage, &out_dir_var, true);

    let containerfile_path = md.finalize(&green, &target_path, krate_name, &mds)?;

    let fallback = async move {
        let mut cmd = Command::new(&exe);
        let cmd = cmd.kill_on_drop(true);
        // Do not unset ENV_EXECUTE_BUILDRS
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
