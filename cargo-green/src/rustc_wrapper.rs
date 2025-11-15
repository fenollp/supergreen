use std::{
    collections::{BTreeMap, HashMap},
    env,
    fs::{self},
    future::Future,
};

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, error, info, trace, warn};
use tokio::process::Command;

use crate::{
    build::{Effects, ERRCODE, STDERR, STDOUT},
    checkouts,
    cratesio::{self, rewrite_cratesio_index},
    ext::CommandExt,
    green::Green,
    logging::{self, maybe_log},
    md::{get_or_read, BuildContext, Md, MdId, MountExtern, NamedMount},
    pwd,
    runner::Runner,
    rustc_arguments::{as_rustc, RustcArgs},
    stage::{Stage, RST, RUST},
    tmp, PKG, VSN,
};

pub(crate) const ENV_EXECUTE_BUILDRS: &str = "CARGOGREEN_EXECUTE_BUILDRS_";
const WRAP_BUILDRS: bool = true; // FIXME: finish experiment

// NOTE: this RUSTC_WRAPPER program only ever gets called by `cargo`, so we save
//       ourselves some trouble and assume std::path::{Path, PathBuf} are UTF-8.

macro_rules! ENV {
    () => {
        "CARGOGREEN"
    };
}

pub(crate) async fn main(
    green: Green,
    arg0: Option<String>,
    args: Vec<String>,
    vars: BTreeMap<String, String>,
) -> Result<()> {
    let argz = args.iter().take(3).map(AsRef::as_ref).collect::<Vec<_>>();

    let argv = |times| args.clone().into_iter().skip(times).collect();

    // TODO: find a better heuristic to ensure `rustc` is rustc
    match &argz[..] {
        [rustc, "--crate-name", crate_name, ..] if rustc.ends_with("rustc") =>
             wrap_rustc(green, crate_name, argv(1), call_rustc(rustc, argv(1))).await,
        [driver, rustc, "-"|"--crate-name", ..] if rustc.ends_with("rustc") => {
            // TODO: wrap driver? + rustc
            // driver: e.g. $RUSTUP_HOME/toolchains/stable-x86_64-unknown-linux-gnu/bin/clippy-driver
            // cf. https://github.com/rust-lang/rust-clippy/tree/da27c979e29e78362b7a2a91ebcf605cb01da94c#using-clippy-driver
             call_rustc(driver, argv(2)).await
         }
        [_driver, rustc, ..] if rustc.ends_with("rustc") =>
             call_rustc(rustc, argv(2)).await,
        [rustc, ..] if rustc.ends_with("rustc") =>
             call_rustc(rustc, argv(1)).await,
        _ => panic!("RUSTC_WRAPPER={arg0:?}'s input unexpected:\n\targz = {argz:?}\n\targs = {args:?}\n\tenvs = {vars:?}\n"),
    }
}

#[test]
fn passthrough_getting_rust_target_specific_information() {
    #[rustfmt::skip]
    let first_few_args = &[
        "$PWD/rustcbuildx/rustcbuildx",
        "$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc",
        "-",
        "--crate-name", "___",
        "--print=file-names",
        "--crate-type", "bin",
        "--crate-type", "rlib",
        "--crate-type", "dylib",
        "--crate-type", "cdylib",
        "--crate-type", "staticlib",
        "--crate-type", "proc-macro",
        "--print=sysroot",
        "--print=split-debuginfo",
        "--print=crate-name",
        "--print=cfg",
    ]
    .into_iter()
    .take(4)
    .map(ToOwned::to_owned)
    .collect::<Vec<String>>();

    let first_few_args =
        first_few_args.iter().skip(1).take(3).map(String::as_str).collect::<Vec<_>>();
    assert_eq!(
        match &first_few_args[..] {
            [_rustc, "-", ..] | [_rustc, _ /*driver*/, "-", ..] => 1,
            [_rustc, "--crate-name", _crate_name, ..] => 2,
            _ => 3,
        },
        1
    );
}

/// NOTE: not running inside Docker: local install SHOULD match Docker image setup
/// Meaning: it's up to the user to craft their desired $CARGOGREEN_BASE_IMAGE
async fn call_rustc(rustc: &str, args: Vec<String>) -> Result<()> {
    let mut cmd = Command::new(rustc);
    let cmd = cmd.kill_on_drop(true).args(args);
    let status = cmd
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn {}: {e}", cmd.show()))?
        .wait()
        .await
        .map_err(|e| anyhow!("Failed to wait {}: {e}", cmd.show()))?;
    if !status.success() {
        bail!("Failed in call_rustc")
    }
    Ok(())
}

pub(crate) async fn exec_buildrs(green: Green, exe: Utf8PathBuf) -> Result<()> {
    assert!(env::var_os(ENV!()).is_none(), "It's turtles all the way down!");
    env::set_var(ENV!(), "1");

    let krate_name = env::var("CARGO_PKG_NAME").expect("$CARGO_PKG_NAME");
    let krate_version = env::var("CARGO_PKG_VERSION").expect("$CARGO_PKG_VERSION");

    // exe: /target/release/build/proc-macro2-2f938e044e3f79bf/build-script-build
    let Some((previous_md_path, previous_extra)) = || -> Option<_> {
        // target_path: /target/release/build/proc-macro2-2f938e044e3f79bf
        let target_path = exe.parent()?;

        // extra: -2f938e044e3f79bf
        let extra = format!("-{}", target_path.file_name()?.rsplit('-').next()?);

        // target_path: /target/release
        let target_path = target_path.parent()?.parent()?;

        // /target/release/2f938e044e3f79bf.toml
        Some((MdId::new(&extra).path(target_path), extra))
    }() else {
        bail!("BUG: malformed buildrs exe {exe:?}")
    };

    // $OUT_DIR: /target/release/build/proc-macro2-b97492fdd0201a99/out
    let out_dir_var: Utf8PathBuf = env::var("OUT_DIR").expect("$OUT_DIR").into();
    let Some((md_path, extra)) = || -> Option<_> {
        // name: proc-macro2-b97492fdd0201a99
        let name = out_dir_var.parent()?.file_name()?;

        // extra: -b97492fdd0201a99
        let extra = format!("-{}", name.rsplit('-').next()?);

        // /target/release/b97492fdd0201a99.toml
        Some((previous_md_path.with_file_name(format!("{}.toml", MdId::new(&extra))), extra))
    }() else {
        bail!("BUG: malformed $OUT_DIR {out_dir_var:?}")
    };

    // Z: for executing build scripts
    let full_krate_id = format!("Z {krate_name} {krate_version}{extra}");
    logging::setup(&full_krate_id);

    info!("{PKG}@{VSN} original args: {exe:?} green={green:?}");

    do_exec_buildrs(
        green,
        &krate_name,
        // krate_version,
        full_krate_id.replace(' ', "-"),
        out_dir_var,
        exe,
        previous_md_path,
        previous_extra,
        md_path,
        extra,
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
    previous_md_path: Utf8PathBuf,
    previous_extra: String,
    md_path: Utf8PathBuf,
    extra: String,
) -> Result<()> {
    let debug = maybe_log();

    let run_stage = Stage::try_new(format!("run-{crate_id}"))?;
    let out_stage = Stage::try_new(format!("ran{extra}"))?;

    // let code_stage = Stage::try_new(format!("cratesio-{krate_name}-{krate_version}"))?; // FIXME
    // let code_mount_src = "/extracted"; //FIXME
    // let code_mount_dst = format!("/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/{krate_name}-{krate_version}"); //FIXME

    let previous_out_stage = Stage::try_new(format!("out{previous_extra}"))?; //FIXME
    let previous_out_dst = {
        let name = exe.file_name().expect("PROOF: already ensured path has file_name");
        let name = name.replacen('-', "_", 2);
        format!("/{name}{previous_extra}")
    };

    let mut md = Md::new(&extra);
    md.push_block(&RUST, green.base.image_inline.clone().unwrap());

    fs::create_dir_all(&out_dir_var)
        .map_err(|e| anyhow!("Failed to `mkdir -p {out_dir_var}`: {e}"))?;

    let mut run_block = String::new();
    run_block.push_str(&format!("FROM {RST} AS {run_stage}\n"));
    // run_block.push_str(&format!("SHELL {:?}\n", ["/bin/bash", "-eux", "-c"]));
    run_block.push_str(&format!("SHELL {:?}\n", ["/bin/bash", "-euxo", "pipefail", "-c"]));
    run_block.push_str(&format!("WORKDIR {out_dir_var}\n"));
    run_block.push_str("RUN \\\n");
    // run_block.push_str(&format!(
    //     "  --mount=from={code_stage},source={code_mount_src},dst={code_mount_dst} \\\n"
    // ));
    run_block.push_str(&format!(
        "  --mount=from={previous_out_stage},source={previous_out_dst},dst={exe} \\\n"
    ));

    let mut mds = HashMap::<Utf8PathBuf, Md>::new(); // A file cache FIXME: merge both into Mds

    let previous_md = get_or_read(&mut mds, &previous_md_path)?;
    trace!(">>>previous_md_path = {previous_md_path}");
    trace!(">>>previous_md      = {previous_md:?}");
    // let target_path = previous_md_path.parent().unwrap();
    // let (mounts, mut mds) =
    //     assemble_build_dependencies(&mut md, "bin", "dep-info,link", [].into(), target_path)?;
    // mds.push(previous_md);
    // for NamedMount { name, src, dst } in mounts {
    //     run_block.push_str(&format!("  --mount=from={name},dst={dst},source={src} \\\n"));
    // }

    let mut extern_mds_and_paths: Vec<_> = previous_md
        .short_externs
        .iter()
        .map(|xtern| -> Result<_> {
            let xtern_md_path = previous_md_path.with_file_name(format!("{xtern}.toml"));
            let xtern_md = get_or_read(&mut mds, &xtern_md_path)?;
            Ok((xtern_md_path, xtern_md))
        })
        .collect::<Result<_>>()?;
    // md.short_externs.push(previous_md as short xtern) FIXME?? MAY counter assemble+outdirvar
    extern_mds_and_paths.push((previous_md_path, previous_md));
    let extern_md_paths = md.sort_deps(extern_mds_and_paths)?;
    info!("extern_md_paths: {}", extern_md_paths.len());

    let mds = extern_md_paths
        .into_iter()
        .map(|extern_md_path| get_or_read(&mut mds, &extern_md_path))
        .collect::<Result<Vec<_>>>()?;

    run_block.push_str(&format!("    env CARGO={:?} \\\n", "$(which cargo)"));
    for (var, val) in env::vars().filter_map(|kv| fmap_env(kv, true)) {
        run_block.push_str(&format!("        {var}={val} \\\n"));
    }
    run_block.push_str(&format!("        {}=1 \\\n", ENV!()));
    for var in &green.set_envs {
        if let Some(val) = env::var_os(var) {
            warn!("passing ${var}={val:?} env through");
            run_block.push_str(&format!("        {var}={val:?} \\\n"));
        }
    }
    run_block.push_str(&format!("        {ENV_EXECUTE_BUILDRS}= \\\n"));
    run_block.push_str(&format!("      {exe} \\\n"));
    run_block.push_str(&format!("        1> >(tee    {out_dir_var}/{out_stage}-{STDOUT}) \\\n"));
    run_block
        .push_str(&format!("        2> >(tee    {out_dir_var}/{out_stage}-{STDERR} >&2) \\\n"));
    run_block.push_str(&format!("        || echo $? >{out_dir_var}/{out_stage}-{ERRCODE}\n"));
    md.push_block(&run_stage, run_block);

    let mut out_block = String::new();
    out_block.push_str(&format!("FROM scratch AS {out_stage}\n"));
    out_block.push_str(&format!("COPY --from={run_stage} {out_dir_var}/* /\n"));
    // out_block.push_str(&format!("COPY --from={run_stage} {out_dir_var}/*{extra}* /\n"));
    md.push_block(&out_stage, out_block);

    // // let containerfile_path = md_path.with_extension("Dockerfile");
    // let md_path = md.this().path(&target_path);
    // let containerfile_path = target_path.join(format!("{krate_name}{extra}.Dockerfile"));
    let containerfile_path = md_path.with_file_name(format!("{krate_name}{extra}.Dockerfile"));

    md.write_to(&md_path)?;

    let mut containerfile = green.new_containerfile();
    containerfile.pushln(&md.rust_stage());
    containerfile.nl();
    containerfile.push(&md.block_along_with_predecessors(&mds));
    containerfile.write_to(&containerfile_path)?;
    drop(containerfile);

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

    if green.runner == Runner::None {
        info!("Runner disabled, falling back...");
        return fallback.await;
    }
    let (_call, _envs, ran) =
        green.build_out(&containerfile_path, &out_stage, &md.contexts, &out_dir_var).await;
    match ran {
        Ok(Effects { written, stdout, stderr }) => {
            debug!(">>> written={written:?}");
            debug!(">>> stdout={stdout:?}");
            debug!(">>> stderr={stderr:?}");
            Ok(())
        }
        Err(e) if debug.is_none() => {
            warn!("Falling back due to {e}");
            // Bubble up actual error & outputs
            fallback
                .await
                .inspect(|()| eprintln!("BUG: {PKG} should not have encountered this error: {e}"))
        }
        Err(e) => Err(e),
    }
}

async fn wrap_rustc(
    green: Green,
    crate_name: &str,
    arguments: Vec<String>,
    fallback: impl Future<Output = Result<()>>,
) -> Result<()> {
    assert!(env::var_os(ENV!()).is_none(), "It's turtles all the way down!");
    env::set_var(ENV!(), "1");

    let pwd = pwd();

    let out_dir_var = env::var("OUT_DIR").ok().map(Utf8PathBuf::from);

    let (st, args) = as_rustc(&pwd, &arguments, out_dir_var.as_deref())?;

    let buildrs = ["build_script_build", "build_script_main"].contains(&crate_name);
    // NOTE: krate_name != crate_name: Gets named build_script_build + s/-/_/g + may actually be a different name
    let krate_name = env::var("CARGO_PKG_NAME").expect("$CARGO_PKG_NAME");

    let krate_version = env::var("CARGO_PKG_VERSION").expect("$CARGO_PKG_VERSION");

    let krate_manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("$CARGO_MANIFEST_DIR");
    let krate_manifest_dir = Utf8Path::new(&krate_manifest_dir);

    let krate_repository = env::var("CARGO_PKG_REPOSITORY").ok().unwrap_or_default();

    let full_krate_id = {
        // X: for building build scripts
        let kind = if buildrs { 'X' } else { 'N' }; // exe or normal
        format!("{kind} {krate_name} {krate_version}{}", st.extrafn)
    };

    logging::setup(&full_krate_id);

    info!("{PKG}@{VSN} original args: {arguments:?} pwd={pwd} st={st:?} green={green:?}");

    do_wrap_rustc(
        green,
        &krate_name,
        krate_version,
        krate_manifest_dir,
        krate_repository,
        buildrs,
        full_krate_id.replace(' ', "-"),
        pwd,
        args,
        out_dir_var,
        st,
        fallback,
    )
    .await
    .inspect_err(|e| error!("Error: {e}"))
}

#[expect(clippy::too_many_arguments)]
async fn do_wrap_rustc(
    green: Green,
    krate_name: &str,
    krate_version: String,
    krate_manifest_dir: &Utf8Path,
    krate_repository: String,
    buildrs: bool,
    crate_id: String,
    pwd: Utf8PathBuf,
    args: Vec<String>,
    out_dir_var: Option<Utf8PathBuf>,
    RustcArgs { externs, extrafn, incremental, input, out_dir, target_path }: RustcArgs,
    fallback: impl Future<Output = Result<()>>,
) -> Result<()> {
    let debug = maybe_log();

    let incremental = green.incremental().then_some(incremental).flatten();

    let mut md = Md::new(&extrafn);
    md.push_block(&RUST, green.base.image_inline.clone().unwrap());

    fs::create_dir_all(&out_dir).map_err(|e| anyhow!("Failed to `mkdir -p {out_dir}`: {e}"))?;
    if let Some(ref incremental) = incremental {
        fs::create_dir_all(incremental)
            .map_err(|e| anyhow!("Failed to `mkdir -p {incremental}`: {e}"))?;
    }

    let cargo_home: Utf8PathBuf = home::cargo_home()
        .map_err(|e| anyhow!("bad $CARGO_HOME or something: {e}"))?
        .try_into()
        .map_err(|e| anyhow!("corrupted $CARGO_HOME path: {e}"))?;

    // TODO: support non-crates.io crates managers + proxies
    // TODO: use --secret mounts for private deps (and secret direct artifacts)
    let (input_mount, rustc_stage) = if input.starts_with(cargo_home.join("registry/src")) {
        // Input is of a crate dep (hosted at crates.io)
        // Let's optimize this case by fetching & caching crate tarball

        let (stage, src, dst, block) =
            cratesio::into_stage(&cargo_home, krate_name, &krate_version, krate_manifest_dir)
                .await?;
        md.push_block(&stage, block);

        (Some((stage, Some(src), dst)), Stage::dep(&crate_id)?)
    } else if !krate_repository.is_empty()
        && krate_manifest_dir.starts_with(cargo_home.join("git/checkouts"))
    {
        // Input is of a git checked out dep

        let (stage, dst, block) =
            checkouts::into_stage(krate_manifest_dir, &krate_repository).await?;
        md.push_block(&stage, block);

        (Some((stage, None, dst)), Stage::dep(&crate_id)?)
    } else {
        // Input is local code

        assert!(input.is_relative(), "BUG: input isn't relative: {input:?}");
        (None, Stage::local(&crate_id)?)
    };
    info!("picked {rustc_stage} for {input}");
    let input = rewrite_cratesio_index(&input);

    let incremental_stage = Stage::incremental(md.this())?;
    let out_stage = Stage::output(md.this())?;

    let mut rustc_block = String::new();
    rustc_block.push_str(&format!("FROM {RST} AS {rustc_stage}\n"));
    rustc_block.push_str(&format!("SHELL {:?}\n", ["/bin/bash", "-euxo", "pipefail", "-c"]));
    rustc_block.push_str(&format!("WORKDIR {out_dir}\n"));
    if !pwd.starts_with(cargo_home.join("registry/src")) {
        // Essentially match the same-ish path that points to crates-io paths.
        // Experiment showed that git-check'ed-out crates didn't like: // if !pwd.starts_with(&cargo_home) {
        rustc_block.push_str(&format!("WORKDIR {pwd}\n"));
    }

    if let Some(ref incremental) = incremental {
        rustc_block.push_str(&format!("WORKDIR {incremental}\n"));
    }

    let cwd = if let Some((name, src, dst)) = input_mount.as_ref() {
        rustc_block.push_str("RUN \\\n");
        let source = src.as_deref().map(|src| format!(",source={src}")).unwrap_or_default();
        let rw = if buildrs { ",rw" } else { "" };
        //FIXME: rw is probs just ducktape and an actual stage is needed to keep results
        rustc_block.push_str(&format!("  --mount=from={name}{source},dst={dst}{rw} \\\n"));

        None
    } else {
        // NOTE: we don't `rm -rf cwd_root`
        let cwd_root = tmp().join(format!("{PKG}_{VSN}"));
        fs::create_dir_all(&cwd_root)
            .map_err(|e| anyhow!("Failed `mkdir -p {cwd_root:?}`: {e}"))?;

        let ignore = cwd_root.join(".dockerignore");
        fs::write(&ignore, "")
            .map_err(|e| anyhow!("Failed creating cwd dockerignore {ignore:?}: {e}"))?;

        let cwd_path = cwd_root.join(format!("CWD{extrafn}"));

        info!(
            "copying all {}files under {pwd} to {cwd_path}",
            if pwd.join(".git").is_dir() { "git " } else { "" }
        );

        copy_dir_all(&pwd, &cwd_path)?; //TODO: atomic mv: https://github.com/untitaker/rust-atomicwrites

        // TODO: --mount=bind each file one by one => drop temp dir ctx (needs [multiple] `mkdir -p`[s] first though)
        // This doesn't work: rustc_block.push_str(&format!("  --mount=from=cwd,dst={pwd} \\\n"));
        // ✖ 0.040 runc run failed: unable to start container process: error during container init:
        //     error mounting "/var/lib/docker/tmp/buildkit-mount1189821268/libaho_corasick-b99b6e1b4f09cbff.rlib"
        //     to rootfs at "/home/runner/work/rustcbuildx/rustcbuildx/target/debug/deps/libaho_corasick-b99b6e1b4f09cbff.rlib":
        //         mkdir /var/lib/docker/buildkit/executor/m7p2ehjfewlxfi5zjupw23oo7/rootfs/home/runner/work/rustcbuildx/rustcbuildx/target:
        //             read-only file system
        // Meaning: tried to mount overlapping paths
        // TODO: try mounting each individual file from `*.d` dep file
        // 0 0s debug HEAD λ cat rustcbuildx.d
        // $target_dir/debug/rustcbuildx: $cwd/src/cli.rs $cwd/src/cratesio.rs $cwd/src/envs.rs $cwd/src/main.rs $cwd/src/md.rs $cwd/src/parse.rs $cwd/src/pops.rs $cwd/src/runner.rs $cwd/src/stage.rs

        // TODO: do better to avoid copying >1 times local work dir on each cargo call => context-mount local content-addressed tarball?
        // test|cargo-green|0.8.0|f273b3fc9f002200] copying all git files under $HOME/wefwefwef/supergreen.git to /tmp/cargo-green_0.8.0/CWDf273b3fc9f002200
        // bin|cargo-green|0.8.0|efe5575298075b07] copying all git files under $HOME/wefwefwef/supergreen.git to /tmp/cargo-green_0.8.0/CWDefe5575298075b07
        let cwd_stage = Stage::local_mount(md.this())?;

        rustc_block.push_str(&format!("COPY --link --from={cwd_stage} / .\n"));
        rustc_block.push_str("RUN \\\n");

        Some((cwd_stage, cwd_path))
    };

    md.contexts = [cwd]
        .into_iter()
        .flatten()
        .map(|(name, uri)| BuildContext { name, uri })
        .inspect(|BuildContext { name, uri }| info!("loading {name:?}: {uri}"))
        .collect();
    info!("loading {} build contexts", md.contexts.len());

    let mds = md.assemble_build_dependencies(externs, out_dir_var, &target_path)?;
    for MountExtern { from, xtern } in md.externs() {
        let dst = target_path.join("deps").join(xtern);
        rustc_block.push_str(&format!("  --mount=from={from},dst={dst},source=/{xtern} \\\n"));
    }
    for NamedMount { name, src, dst } in &md.mounts {
        //FIXME: no need to mount as writeable or to make a stage?
        rustc_block.push_str(&format!("  --mount=from={name},dst={dst},source={src} \\\n"));
    }

    // Log a possible toolchain file contents (TODO: make per-crate base.image out of this)
    if false {
        rustc_block.push_str("    { cat ./rustc-toolchain{,.toml} 2>/dev/null || true ; } && \\\n");
    }

    if WRAP_BUILDRS && buildrs {
        // TODO: {extrafn} STDIO consts
        // TODO: this won't work with e.g. tokio-decorated main fns (async + decorator needs duplicating)

        rustc_block.push_str(&format!(
            r#"    {{ \
        cat {input} | sed 's/fn main/fn actual{uniq}_main/' >{input}~ && mv {input}~ {input} ; \
        {{ \
          echo ; \
          echo 'fn main() {{' ; \
          echo '    use std::env::{{args_os, var_os}};' ; \
          echo '    if var_os("{ENV_EXECUTE_BUILDRS}").is_none() {{' ; \
          echo '        use std::process::{{Command, Stdio}};' ; \
          echo '        let mut cmd = Command::new("{PKG}");' ; \
          echo '        cmd.stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit());' ; \
          echo '        cmd.env("{ENV_EXECUTE_BUILDRS}", args_os().next().expect("{PKG}: getting buildrs arg0"));' ; \
          echo '        let res = cmd.spawn().expect("{PKG}: spawning buildrs").wait().expect("{PKG}: running builds");' ; \
          echo '        assert!(res.success());' ; \
          echo '    }} else {{' ; \
          echo '        actual{uniq}_main()' ; \
          echo '    }}' ; \
          echo '}}' ; \
        }} >>{input} ; \
    }} && \
"#,
            uniq = extrafn.replace('-', "_"),
        ));
    }

    rustc_block.push_str(&format!("    env CARGO={:?} \\\n", "$(which cargo)"));

    for (var, val) in env::vars().filter_map(|kv| fmap_env(kv, buildrs)) {
        rustc_block.push_str(&format!("        {var}={val} \\\n"));
    }
    rustc_block.push_str(&format!("        {}=1 \\\n", ENV!()));
    // => cargo upstream issue "pass env vars read/wrote by build script on call to rustc"
    // TODO whence https://github.com/rust-lang/cargo/issues/14444#issuecomment-2305891696
    for var in &green.set_envs {
        if let Some(val) = env::var_os(var) {
            warn!("passing ${var}={val:?} env through");
            rustc_block.push_str(&format!("        {var}={val:?} \\\n"));
        }
    }
    // TODO: catch these cargo:rustc-env= to add to TOML+Dockerfile in extremis so downstream knows about these envs
    // 2025-04-05T09:42:41.5322589Z [typenum 1.12.0] cargo:rustc-env=TYPENUM_BUILD_CONSTS=/home/runner/instst/release/build/typenum-3cf9e442dfddd505/out/consts.rs
    // 2025-04-05T09:42:41.5748814Z [typenum 1.12.0] cargo:rustc-env=TYPENUM_BUILD_OP=/home/runner/instst/release/build/typenum-3cf9e442dfddd505/out/op.rs
    // https://doc.rust-lang.org/cargo/reference/build-scripts.html#outputs-of-the-build-script
    // https://github.com/ALinuxPerson/build_script?tab=readme-ov-file#examples
    // TODO: also maybe same for "rustc wrote"?

    // TODO: keep only paths that we explicitly mount or copy
    if false {
        // https://github.com/maelstrom-software/maelstrom/blob/ef90f8a990722352e55ef1a2f219ef0fc77e7c8c/crates/maelstrom-util/src/elf.rs#L4
        for var in ["PATH", "DYLD_FALLBACK_LIBRARY_PATH", "LD_LIBRARY_PATH", "LIBPATH"] {
            let Ok(val) = env::var(var) else { continue };
            debug!("system env set (skipped): ${var}={val:?}");
            if !val.is_empty() && debug.is_some() {
                rustc_block.push_str(&format!("#       {var}={val:?} \\\n"));
            }
        }
    }

    rustc_block.push_str(&format!("      rustc '{}' {input} \\\n", args.join("' '")));
    rustc_block.push_str(&format!("        1>          {out_dir}/{out_stage}-{STDOUT} \\\n"));
    rustc_block.push_str(&format!("        2>          {out_dir}/{out_stage}-{STDERR} \\\n"));
    rustc_block.push_str(&format!("        || echo $? >{out_dir}/{out_stage}-{ERRCODE}\\\n"));
    // TODO: [`COPY --rewrite-timestamp ...` to apply SOURCE_DATE_EPOCH build arg value to the timestamps of the files](https://github.com/moby/buildkit/issues/6348)
    rustc_block.push_str(&format!("  ; find {out_dir}/*{extrafn}* -print0 | xargs -0 touch --no-dereference --date=@$SOURCE_DATE_EPOCH\n"));
    md.push_block(&rustc_stage, rustc_block);

    if let Some(ref incremental) = incremental {
        let mut incremental_block = format!("FROM scratch AS {incremental_stage}\n");
        incremental_block.push_str(&format!("COPY --link --from={rustc_stage} {incremental} /\n"));
        md.push_block(&incremental_stage, incremental_block);
    }

    let mut out_block = format!("FROM scratch AS {out_stage}\n");
    out_block.push_str(&format!("COPY --link --from={rustc_stage} {out_dir}/*{extrafn}* /\n"));
    md.push_block(&out_stage, out_block);
    // TODO? in Dockerfile, when using outputs:
    // => skip the COPY (--mount=from=out-08c4d63ed4366a99)
    //   => use the stage directly (--mount=from=dep-l-buildxargs-1.4.0-08c4d63ed4366a99)

    let md_path = md.this().path(&target_path);
    let containerfile_path = target_path.join(format!("{krate_name}{extrafn}.Dockerfile"));

    md.write_to(&md_path)?;

    let mut containerfile = green.new_containerfile();
    containerfile.pushln(&md.rust_stage());
    containerfile.nl();
    containerfile.push(&md.block_along_with_predecessors(&mds));
    containerfile.write_to(&containerfile_path)?;
    drop(containerfile);

    // TODO: use tracing instead:
    // https://docs.rs/tracing-subscriber/latest/tracing_subscriber/fmt/struct.Subscriber.html
    // https://crates.io/crates/tracing-appender
    // https://github.com/tugglecore/rust-tracing-primer
    // TODO: `cargo green -v{N+1} ..` starts a TUI showing colored logs on above `cargo -v{N} ..`

    if green.runner == Runner::None {
        info!("Runner disabled, falling back...");
        return fallback.await;
    }

    let contexts = &md.contexts;
    let build = |stage, dir| green.build_out(&containerfile_path, stage, contexts, dir);
    let (call, envs, built) = build(&out_stage, &out_dir).await;
    green
        .maybe_write_final_path(&containerfile_path, contexts, &call, &envs)
        .map_err(|e| anyhow!("Failed producing final path: {e}"))?;

    match built {
        Ok(Effects { written, stdout, stderr }) => {
            if !written.is_empty() || !stdout.is_empty() || !stderr.is_empty() {
                md.writes = written;
                md.stdout = stdout;
                md.stderr = stderr;
                info!("re-opening (RW) crate's md {md_path}");
                md.write_to(&md_path)?;
            }

            let final_stage = format!(
                "FROM scratch\n{}\n",
                md.writes
                    .iter()
                    .filter_map(|f| f.file_name())
                    .filter(|f| !f.ends_with(".d"))
                    .filter(|f| f != &format!("{out_stage}-{STDOUT}"))
                    .filter(|f| f != &format!("{out_stage}-{STDERR}"))
                    .filter(|f| f != &format!("{out_stage}-{ERRCODE}"))
                    .map(|f| (f, f.replace(&extrafn, "")))
                    .map(|(src, dst)| format!("COPY --link --from={out_stage} /{src} /{dst}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );

            green
                .maybe_append_to_final_path(&md_path, final_stage)
                .map_err(|e| anyhow!("Failed finishing final path: {e}"))?;
        }
        Err(e) if debug.is_none() => {
            warn!("Falling back due to {e}");
            // Bubble up actual error & outputs
            return fallback
                .await
                .inspect(|()| eprintln!("BUG: {PKG} should not have encountered this error: {e}"));
        }
        Err(e) => return Err(e),
    }

    if let Some(incremental) = incremental {
        if let (_, _, Err(e)) = build(&incremental_stage, &incremental).await {
            warn!("Error building incremental data: {e}");
            return Err(e);
        }
    }

    Ok(())
}

fn fmap_env((var, val): (String, String), buildrs: bool) -> Option<(String, String)> {
    let (pass, skip, only_buildrs) = pass_env(&var);
    if pass || (buildrs && only_buildrs) {
        if skip {
            debug!("not forwarding env: {var}={val}");
            return None;
        }
        let val = safeify(val);
        if var == "CARGO_ENCODED_RUSTFLAGS" {
            let dec: Vec<_> = rustflags::from_env().collect();
            debug!("env is set: {var}={val} ({dec:?})");
        } else {
            debug!("env is set: {var}={val}");
        }
        let val = match var.as_str() {
            // "CARGO_PKG_DESCRIPTION" => "FIXME".to_owned(),
            "CARGO_MANIFEST_DIR" | "CARGO_MANIFEST_PATH" => {
                rewrite_cratesio_index(Utf8Path::new(&val)).to_string()
            }
            "TERM" => return None,
            "RUSTC" => "rustc".to_owned(), // Rewrite host rustc so the base_image one can be used
            // "CARGO_TARGET_DIR" | "CARGO_BUILD_TARGET_DIR" => {
            //     virtual_target_dir(Utf8Path::new(&val)).to_string()
            // }
            // // TODO: a constant $CARGO_TARGET_DIR possible solution is to wrap build script as it runs,
            // // ie. controlling all outputs. This should help: https://github.com/trailofbits/build-wrap/blob/d7f43b76e655e43755f68e28e9d729b4ed1dd115/src/wrapper.rs#L29
            // //(dbcc)=> Dirty typenum v1.12.0: stale, https://github.com/rust-lang/cargo/blob/7987d4bfe683267ba179b42af55891badde3ccbf/src/cargo/core/compiler/fingerprint/mod.rs#L2030
            // //=> /tmp/clis-dbcc_2-2-1/release/deps/typenum-32188cb0392f25b9.d
            // "OUT_DIR" => virtual_target_dir(Utf8Path::new(&val)).to_string(),
            "CARGO_TARGET_DIR" | "CARGO_BUILD_TARGET_DIR" => return None,
            "OUT_DIR" => val,
            _ => val,
        };
        return Some((var, val));
    }
    None
}

// https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates
#[must_use]
fn pass_env(var: &str) -> (bool, bool, bool) {
    // Thanks https://github.com/cross-rs/cross/blob/44011c8854cb2eaac83b173cc323220ccdff18ea/src/docker/shared.rs#L969
    let passthrough = [
        "http_proxy",
        "TERM",
        "RUSTDOCFLAGS",
        "RUSTFLAGS",
        "BROWSER",
        "HTTPS_PROXY",
        "HTTP_TIMEOUT",
        "https_proxy",
        "QEMU_STRACE",
        // Not here but set in RUN script: CARGO, PATH, ...
        "OUT_DIR", // (Only set during compilation.)
    ];
    // TODO: vvv drop what can be dropped vvv
    let skiplist = [
        "CARGO_BUILD_JOBS",
        "CARGO_BUILD_RUSTC",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTDOC",
        "CARGO_BUILD_TARGET_DIR",
        "CARGO_HOME",      // TODO? drop
        "CARGO_MAKEFLAGS", // TODO: probably drop
        "CARGO_TARGET_DIR",
        "LD_LIBRARY_PATH", // TODO: probably drop
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
    ];
    let buildrs_only = [
        "DEBUG",
        "HOST",
        "NUM_JOBS",
        "OPT_LEVEL",
        "OUT_DIR",
        "PROFILE",
        "RUSTC",
        "RUSTC_LINKER",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTDOC",
        "TARGET",
    ];
    (
        var.starts_with("CARGO_") || passthrough.contains(&var),
        skiplist.contains(&var),
        var.starts_with("DEP_") || buildrs_only.contains(&var),
    )
}

#[must_use]
fn safeify(val: String) -> String {
    (!val.is_empty())
        .then_some(val)
        .map(|x: String| format!("{x:?}"))
        .unwrap_or_default()
        .replace('$', "\\$")
}

#[test]
fn test_safeify() {
    assert_eq!(safeify("$VAR=val".to_owned()), "\"\\$VAR=val\"".to_owned());
}

fn copy_dir_all(src: &Utf8Path, dst: &Utf8Path) -> Result<()> {
    debug!("copy_dir_all: checking (RO) {dst}");
    if dst.exists() {
        return Ok(());
    }

    // Heuristic: test for existence of ./target/CACHEDIR.TAG
    // https://bford.info/cachedir/
    let cachedir = src.join("CACHEDIR.TAG");
    debug!("copy_dir_all: checking (RO) {cachedir}");
    if cachedir.exists() {
        return Ok(()); // Skip copying ./target dir
    }

    fs::create_dir_all(dst).map_err(|e| anyhow!("Failed `mkdir -p {dst:?}`: {e}"))?;

    // TODO: deterministic iteration
    for entry in fs::read_dir(src).map_err(|e| anyhow!("Failed reading dir {src:?}: {e}"))? {
        let entry = entry?;
        let fpath = entry.path();
        let fpath: Utf8PathBuf = fpath
            .clone()
            .try_into()
            .map_err(|e| anyhow!("copying {fpath:?} found corrupted UTF-8 encoding: {e}"))?;
        let Some(fname) = fpath.file_name() else { return Ok(()) };
        let ty = entry.file_type().map_err(|e| anyhow!("Failed typing {entry:?}: {e}"))?;
        if ty.is_dir() {
            if fname == ".git" {
                continue; // Skip copying .git dir
            }
            copy_dir_all(&fpath, &dst.join(fname))?;
        } else {
            trace!("copying to {:?}: {fpath:?}", dst.join(fname));
            fs::copy(&fpath, dst.join(fname)).map_err(|e| {
                anyhow!("Failed `cp {fpath:?} {dst:?}` ({:?}): {e}", entry.metadata())
            })?;
        }
    }
    Ok(())
}
