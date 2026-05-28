use std::{
    env,
    fs::{self},
    future::Future,
};

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use log::{error, info, warn};

use crate::{
    base_image::rewrite_cargo_home,
    checkouts,
    cratesio::{self, rewrite_cratesio_index},
    dirs::pwd,
    green::Green,
    logging::{self},
    md::{named_mount::NamedMount, BuildContext, Md, MountExtern},
    relative,
    rustc_arguments::{as_rustc, RustcArgs},
    stage::{AsStage, Stage, RST, RUST},
    target_dir::{virtual_target_dir, virtual_target_dir_str},
    wrap::{build_script::is_buildrs_executable, call_config, envs::safeify},
    ENV, PKG, VSN,
};

pub(crate) async fn wrap_rustc(
    green: Green,
    arguments: Vec<String>,
    fallback: impl Future<Output = Result<()>>,
) -> Result<()> {
    assert!(env::var_os(ENV!()).is_none(), "It's turtles all the way down!");
    env::set_var(ENV!(), "1");

    if green.runner.is_none() {
        return fallback.await;
    }

    let pwd = pwd();

    let out_dir_var = env::var("OUT_DIR").ok().map(Utf8PathBuf::from);

    let (st @ RustcArgs { mdid, .. }, args) = as_rustc(&pwd, &arguments, out_dir_var.as_deref())?;

    let (crate_name, pkg_name, pkg_version, pkg_manifest_dir) = call_config();

    let buildrs = crate_name.as_deref().map(is_buildrs_executable).unwrap_or_default();
    let kind = if buildrs { 'X' } else { 'N' }; // building buildrs eXe or Normal
    let full_pkg_id = format!("{kind} {pkg_name} {pkg_version} {mdid}");

    logging::setup(&full_pkg_id);

    info!("{PKG}@{VSN} original args: {arguments:?} pwd={pwd} st={st:?} green={green:?}");

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

    rustc_block.push_str(&format!("WORKDIR {out_dir}\n", out_dir = virtual_target_dir(&out_dir)));
    let not_a_cratesio_crate = !pwd.starts_with(green.cargo_home.join(cratesio::HOME));
    if not_a_cratesio_crate {
        let pwd = virtual_target_dir(&pwd);
        let pwd = rewrite_cargo_home(&green.cargo_home, pwd.as_str());
        rustc_block.push_str(&format!("WORKDIR {pwd}\n"));
    }
    if let Some(ref incremental) = incremental {
        rustc_block.push_str(&format!("WORKDIR {incremental}\n"));
    }

    // TODO: support non-crates.io crates managers + proxies
    // TODO: use --secret mounts for private deps (and secret direct artifacts)
    let mut code_stage = if input.starts_with(green.cargo_home.join(cratesio::HOME)) {
        // Input is of a crate dep (hosted at crates.io)
        // Let's optimize this case by fetching & caching crate tarball

        cratesio::named_stage(&green.cargo_home, pkg_name, pkg_manifest_dir).await?
    } else if pkg_manifest_dir.starts_with(green.cargo_home.join(checkouts::HOME)) {
        // Input is of a git checked out dep

        checkouts::as_stage(&green.cargo_home, pkg_manifest_dir).await?
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
        let dst = virtual_target_dir(&dst);
        let src = src.as_deref().map(|src| format!(",source={src}")).unwrap_or_default();
        let mount = if swappity { format!(",dst={dst}{src}") } else { format!("{src},dst={dst}") };
        rustc_block.push_str(&format!("  --mount=from={name}{mount} \\\n"));
    }

    if let Some((name, uri)) = code_stage.context() {
        info!("loading {name:?}: {uri}");
        md.contexts = [BuildContext { name, uri }].into();
        info!("loading 1 build context");
    }

    let mds = md.assemble_build_dependencies(externs, out_dir_var, &target_path)?;
    for MountExtern { from, xtern } in md.externs() {
        let dst = virtual_target_dir(&target_path).join("deps").join(xtern);
        rustc_block.push_str(&format!("  --mount=from={from},dst={dst},source=/{xtern} \\\n"));
    }
    for NamedMount { name, mount } in &md.mounts {
        rustc_block.push_str(&format!("  --mount=from={name},dst={mount},source=/ \\\n"));
    }

    let out_stage = Stage::output(mdid)?;

    let call = {
        let input = rewrite_cratesio_index(input.as_str());
        let input = rewrite_cargo_home(&green.cargo_home, &input);

        let args = args
            .into_iter()
            .map(|ref x| virtual_target_dir_str(x))
            .map(|arg| safeify(&arg).unwrap())
            .collect::<Vec<_>>()
            .join(" ");

        format!("rustc {args} {input}")
    };
    md.call_block(
        (&rustc_stage, rustc_block),
        crate_name,
        &green.cargo_home,
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

    md.out_block(&out_stage, &rustc_stage, &out_dir, false);

    let (md_path, containerfile_path) = md.finalize(&green, &target_path, pkg_name, &mds)?;

    // TODO: use tracing instead:
    // https://docs.rs/tracing-subscriber/latest/tracing_subscriber/fmt/struct.Subscriber.html
    // https://crates.io/crates/tracing-appender
    // https://github.com/tugglecore/rust-tracing-primer
    // TODO: `cargo green -v{N+1} ..` starts a TUI showing colored logs on above `cargo -v{N} ..`

    md.do_build(&green, &md_path, &containerfile_path, &out_stage, &out_dir).await?;

    if let Some(incremental) = incremental {
        if let (_, _, _, Err(e)) = green
            .build_out(&containerfile_path, &incremental_stage, &md.contexts, &incremental)
            .await
        {
            warn!("Error building incremental data: {e}");
            return Err(e);
        }
    }

    drop(code_stage); // Some impl cleans up files

    Ok(())
}
