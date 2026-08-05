use std::{
    env,
    fs::{self},
    future::Future,
};

use anyhow::{Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use log::{error, info, warn};

use crate::{
    PKG, VSN, checkouts,
    cratesio::{self},
    dirs::{locate_path, pwd},
    green::Green,
    logging::{self},
    md::{BuildContext, Md, NamedMount},
    relative,
    rustc_arguments::{RustcArgs, as_rustc},
    stage::{AsStage, RST, RUST, Stage},
    wrap::{build_script::is_buildrs_executable, call_config, envs::safeify},
};

pub(crate) async fn wrap_rustc(
    green: Green,
    arguments: Vec<String>,
    fallback: impl Future<Output = Result<()>>,
) -> Result<()> {
    let pwd = pwd();

    let out_dir_var = env::var(OUT_DIR!()).ok().map(Utf8PathBuf::from);

    let (st @ RustcArgs { mdid, .. }, args) = as_rustc(&pwd, &arguments, out_dir_var.as_deref())?;
    let mdid = mdid.expect("mdid set");

    let (crate_name, pkg_name, pkg_version, pkg_manifest_dir) = call_config();

    let buildrs = crate_name.as_deref().map(is_buildrs_executable).unwrap_or_default();
    let kind = if buildrs { 'X' } else { 'N' }; // building buildrs eXe or Normal
    let full_pkg_id = format!("{kind} {pkg_name} {pkg_version} {mdid}");

    logging::setup(&full_pkg_id);

    info!("{PKG}@{VSN} original args: {arguments:?} pwd={pwd} st={st:?} green={green:?}");

    if green.runner.is_none() {
        if green.paths.reuse_out(&Stage::output(mdid)?, &st.out_dir).await? {
            return Ok(());
        }
        return fallback.await;
    }

    do_wrap_rustc(
        green,
        crate_name.as_deref(),
        &pkg_name,
        &pkg_manifest_dir,
        Stage::dep(&full_pkg_id.replace(' ', "-"))?,
        pwd,
        args,
        out_dir_var,
        st,
    )
    .await
    .inspect_err(|e| error!("Error: {e}"))
}

#[expect(clippy::too_many_arguments)]
async fn do_wrap_rustc(
    green: Green,
    crate_name: Option<&str>,
    pkg_name: &str,
    pkg_manifest_dir: &Utf8Path,
    rustc_stage: Stage,
    pwd: Utf8PathBuf,
    args: Vec<String>,
    out_dir_var: Option<Utf8PathBuf>,
    RustcArgs { externs, mdid, incremental, input, out_dir, target_path }: RustcArgs,
) -> Result<()> {
    let mdid = mdid.expect("mdid set");
    let mut md: Md = mdid.into();

    md.buildrs = crate_name.map(is_buildrs_executable).unwrap_or_default();
    md.push_block(&RUST, &green.base.image_inline);

    fs::create_dir_all(&out_dir).map_err(|e| anyhow!("Failed to `mkdir -p {out_dir}`: {e}"))?;
    let incremental = green.incremental().then_some(incremental).flatten();
    if let Some(ref incremental) = incremental {
        fs::create_dir_all(incremental)
            .map_err(|e| anyhow!("Failed to `mkdir -p {incremental}`: {e}"))?;
    }

    info!("picked {rustc_stage} for {input}");

    let mut rustc_block = format!("FROM {RST} AS {rustc_stage}\n");

    rustc_block.push_str(&format!("WORKDIR {}\n", green.paths.rewrite_target_dir(&out_dir)));
    let not_a_cratesio_crate = !green.paths.is_cratesio(&pwd);
    if not_a_cratesio_crate {
        rustc_block.push_str(&format!("WORKDIR {}\n", green.paths.rewrite(&pwd)));
    }
    if let Some(ref incremental) = incremental {
        rustc_block.push_str(&format!("WORKDIR {incremental}\n"));
    }

    // TODO: support non-crates.io crates managers + proxies
    // TODO: use --secret mounts for private deps (and secret direct artifacts)
    let mut code_stage = if green.paths.is_cratesio(&input) {
        // Input is of a crate dep (hosted at crates.io)
        // Let's optimize this case by fetching & caching crate tarball

        cratesio::named_stage(&green.paths, pkg_name, pkg_manifest_dir).await?
    } else if green.paths.is_checkout(pkg_manifest_dir) {
        // Input is of a git checked out dep

        checkouts::as_stage(&green.paths, pkg_manifest_dir).await?
    } else if input.is_relative() {
        // Input is local code

        relative::as_stage(mdid, &pwd).await?
    } else {
        bail!("BUG: unhandled input {input:?} ({pkg_manifest_dir})")
    };
    md.push_stage(&code_stage);
    rustc_block.push_str("RUN \\\n");
    for (src, dst, swappity) in code_stage.mounts() {
        let name = code_stage.name();
        let dst = green.paths.rewrite(&dst);
        let src = src.as_deref().map(|src| format!(",source={src}")).unwrap_or_default();
        let mount = if swappity { format!(",dst={dst}{src}") } else { format!("{src},dst={dst}") };
        rustc_block.push_str(&format!("  --mount=from={name}{mount} \\\n"));
    }

    if let Some((name, uri)) = code_stage.context() {
        info!("loading {name:?}: {uri}");
        md.contexts = [BuildContext { name, uri }].into();
        info!("loading 1 build context");
    }

    let mds = md.assemble_build_dependencies(
        &mut green.paths.new_mds_cache(&target_path),
        externs,
        out_dir_var.map(|out_dir| green.paths.rewrite_target_dir(&out_dir)),
    )?;
    for NamedMount { name, mount } in md.externs() {
        let located = locate_path(
            |path| path.join("deps").join(mount),
            &target_path,
            green.paths.host_profile_dir(&target_path).as_deref(),
        );
        let dst = green.paths.rewrite_target_dir(&located);
        rustc_block.push_str(&format!("  --mount=from={name},dst={dst},source=/deps/{mount} \\\n"));
    }
    for NamedMount { name, mount } in &md.mounts {
        let base = mount.file_name().expect("PROOF: OUT_DIR mounts end in /out");
        rustc_block.push_str(&format!("  --mount=from={name},dst={mount},source=/{base} \\\n"));
    }

    let out_stage = Stage::output(mdid)?;

    let call = {
        let input = green.paths.rewrite_str(input.as_str());

        let args = args
            .into_iter()
            .map(|ref x| green.paths.rewrite_target_dir_str(x))
            .map(|arg| safeify(&arg).unwrap())
            .collect::<Vec<_>>()
            .join(" ");

        format!("rustc {args} {input}")
    };
    md.call_block(
        (&rustc_stage, rustc_block),
        crate_name,
        &green.paths,
        &green.set_envs,
        &call,
        (&out_stage, not_a_cratesio_crate.then_some(&out_dir)),
    )?;

    let incremental_stage = Stage::incremental(mdid)?;
    if let Some(ref incremental) = incremental {
        let mut incremental_block = format!("FROM scratch AS {incremental_stage}\n");
        incremental_block.push_str(&format!("COPY --link --from={rustc_stage} {incremental} /\n"));
        md.push_block(&incremental_stage, &incremental_block);
    }

    md.out_block(&out_stage, &rustc_stage, &green.paths, &out_dir);

    let (md_path, containerfile_path) = md.finalize(&green, &target_path, pkg_name, &mds)?;

    // TODO: use tracing instead:
    // https://docs.rs/tracing-subscriber/latest/tracing_subscriber/fmt/struct.Subscriber.html
    // https://crates.io/crates/tracing-appender
    // https://github.com/tugglecore/rust-tracing-primer
    // TODO: `cargo green -v{N+1} ..` starts a TUI showing colored logs on above `cargo -v{N} ..`

    md.do_build(&green, &md_path, &containerfile_path, &out_stage, &out_dir).await?;

    if let Some(incremental) = incremental
        && let (_, _, _, _, Err(e)) = green
            .build_out(&containerfile_path, &incremental_stage, &md.contexts, &incremental)
            .await
    {
        warn!("Error building incremental data: {e}");
        return Err(e);
    }

    drop(code_stage); // Some impl cleans up files

    Ok(())
}
