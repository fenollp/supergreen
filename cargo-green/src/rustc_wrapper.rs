use std::{
    collections::{BTreeMap, HashMap},
    env,
    fs::{self},
    future::Future,
};

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexSet;
use log::{debug, error, info, trace, warn};
use tokio::process::Command;

use crate::{
    checkouts,
    cratesio::{self, rewrite_cratesio_index},
    ext::ShowCmd,
    green::Green,
    logging::{self, crate_type_for_logging, maybe_log},
    md::{BuildContext, Md},
    pwd,
    runner::{build_out, Effects, Runner, MARK_STDERR, MARK_STDOUT},
    rustc_arguments::{as_rustc, RustcArgs},
    stage::{Stage, RST, RUST},
    tmp, PKG, VSN,
};

// NOTE: this RUSTC_WRAPPER program only ever gets called by `cargo`, so we save
//       ourselves some trouble and assume std::path::{Path, PathBuf} are UTF-8.

pub(crate) const ENV: &str = "CARGOGREEN";

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

async fn wrap_rustc(
    green: Green,
    crate_name: &str,
    arguments: Vec<String>,
    fallback: impl Future<Output = Result<()>>,
) -> Result<()> {
    assert!(env::var_os(ENV).is_none(), "It's turtles all the way down!");
    env::set_var(ENV, "1");

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
        let RustcArgs { crate_type, extrafn, .. } = &st;
        if buildrs && crate_type != "bin" {
            bail!("BUG: expected build script to be of crate_type bin, got: {crate_type}")
        }
        let krate_type = if buildrs { 'X' } else { crate_type_for_logging(crate_type) };
        format!("{krate_type} {krate_name} {krate_version}{extrafn}")
    };

    logging::setup(&full_krate_id);

    info!("{PKG}@{VSN} original args: {arguments:?} pwd={pwd} st={st:?} green={green:?}");

    do_wrap_rustc(
        green,
        crate_name,
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

fn crate_out_dir(out_dir_var: Option<Utf8PathBuf>) -> Result<Option<Utf8PathBuf>> {
    let Some(crate_out) = out_dir_var else { return Ok(None) };
    assert_eq!(crate_out.file_name(), Some("out"), "BUG: unexpected $OUT_DIR={crate_out} format");

    info!("listing (RO) crate_out contents {crate_out}");
    let listing = fs::read_dir(&crate_out)
        .map_err(|e| anyhow!("Failed reading crate_out dir {crate_out}: {e}"))?;

    let count = listing
        .map_while(Result::ok)
        .inspect(|f| {
            info!(
                "metadata for {f:?}: {:?}",
                f.metadata().map(|fmd| format!(
                    "created:{c:?} accessed:{a:?} modified:{m:?}",
                    c = fmd.created(),
                    a = fmd.accessed(),
                    m = fmd.modified(),
                ))
            );
        })
        .count();

    // Dir empty => mount can be dropped
    if count == 0 {
        return Ok(None);
    }

    let ignore = crate_out.with_file_name(".dockerignore");
    fs::write(&ignore, "")
        .map_err(|e| anyhow!("Failed creating crate_out dockerignore {ignore}: {e}"))?;

    Ok(Some(crate_out))
}

#[expect(clippy::too_many_arguments)]
async fn do_wrap_rustc(
    green: Green,
    crate_name: &str,
    krate_name: &str,
    krate_version: String,
    krate_manifest_dir: &Utf8Path,
    krate_repository: String,
    buildrs: bool,
    crate_id: String,
    pwd: Utf8PathBuf,
    args: Vec<String>,
    out_dir_var: Option<Utf8PathBuf>,
    RustcArgs { crate_type, emit, externs, extrafn, incremental, input, out_dir, target_path }: RustcArgs,
    fallback: impl Future<Output = Result<()>>,
) -> Result<()> {
    let debug = maybe_log();

    let incremental = green.incremental.then_some(incremental).flatten();

    let crate_out = crate_out_dir(out_dir_var)?;

    let mut md = Md::new(&extrafn[1..]); // Drops leading dash
    md.push_block(&RUST, green.image.base_image_inline.clone().unwrap());

    // This way crates that depend on this know they must require it as .so
    md.is_proc_macro = crate_type == "proc-macro";

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

    let incremental_stage = Stage::incremental(&extrafn)?;
    let out_stage = Stage::output(&extrafn[1..])?; // Drops leading dash

    let mut rustc_block = String::new();
    rustc_block.push_str(&format!("FROM {RST} AS {rustc_stage}\n"));
    rustc_block.push_str(&format!("SHELL {:?}\n", ["/bin/bash", "-eux", "-c"]));
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
        let source = src.map(|src| format!(",source={src}")).unwrap_or_default();
        rustc_block.push_str(&format!("  --mount=from={name}{source},dst={dst} \\\n"));

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

        copy_dir_all(&pwd, &cwd_path)?; //TODO: atomic mv

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
        let cwd_stage = Stage::local_mount(&extrafn)?;

        rustc_block.push_str(&format!("COPY --from={cwd_stage} / .\n"));
        rustc_block.push_str("RUN \\\n");

        Some((cwd_stage, cwd_path))
    };

    if let Some(crate_out) = crate_out.as_deref() {
        let named = crate_out_name(crate_out);
        rustc_block.push_str(&format!("  --mount=from={named},dst={crate_out} \\\n"));
    }

    md.contexts = [cwd, crate_out.map(|crate_out| (crate_out_name(&crate_out), crate_out))]
        .into_iter()
        .flatten()
        .map(|(name, uri)| BuildContext { name, uri })
        .inspect(|BuildContext { name, uri }| info!("loading {name:?}: {uri}"))
        .collect();
    info!("loading {} build contexts", md.contexts.len());

    let (mounts, mds) =
        assemble_build_dependencies(&mut md, &crate_type, &emit, externs, &target_path)?;

    for NamedMount { name, src, dst } in mounts {
        rustc_block.push_str(&format!("  --mount=from={name},dst={dst},source={src} \\\n"));
    }

    // Log a possible toolchain file contents (TODO: make per-crate base_image out of this)
    rustc_block.push_str("    { cat ./rustc-toolchain{,.toml} 2>/dev/null || true ; } && \\\n");
    //fixme? prefix with ::rustc-toolchain::

    rustc_block.push_str(&format!("    env CARGO={:?} \\\n", "$(which cargo)"));

    for (var, val) in env::vars().filter_map(|kv| fmap_env(kv, buildrs)) {
        rustc_block.push_str(&format!("        {var}={val} \\\n"));
    }
    rustc_block.push_str(&format!("        {ENV}=1 \\\n"));
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
    rustc_block.push_str(&format!("        1> >(sed 's/^/{MARK_STDOUT}/') \\\n"));
    rustc_block.push_str(&format!("        2> >(sed 's/^/{MARK_STDERR}/' >&2)\n"));
    md.push_block(&rustc_stage, rustc_block);

    if let Some(ref incremental) = incremental {
        let mut incremental_block = format!("FROM scratch AS {incremental_stage}\n");
        incremental_block.push_str(&format!("COPY --from={rustc_stage} {incremental} /\n"));
        md.push_block(&incremental_stage, incremental_block);
    }

    let mut out_block = format!("FROM scratch AS {out_stage}\n");
    out_block.push_str(&format!("COPY --from={rustc_stage} {out_dir}/*{extrafn}* /\n"));
    md.push_block(&out_stage, out_block);
    // TODO? in Dockerfile, when using outputs:
    // => skip the COPY (--mount=from=out-08c4d63ed4366a99)
    //   => use the stage directly (--mount=from=dep-l-buildxargs-1.4.0-08c4d63ed4366a99)

    // NOTE how we're using BOTH {crate_name} and {krate_name}
    //      => TODO: just use {extrafn}
    //      => TODO: consolidate both as a single file
    let md_path = target_path.join(format!("{crate_name}{extrafn}.toml"));
    let containerfile_path = target_path.join(format!("{krate_name}{extrafn}.Dockerfile"));

    md.write_to(&md_path)?;

    let mut containerfile = green.new_containerfile();
    containerfile.pushln(md.rust_stage());
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
    let build = |stage, dir| build_out(&green, &containerfile_path, stage, &md.contexts, dir);
    match build(out_stage, &out_dir).await {
        Ok(Effects { written }) => {
            if !written.is_empty() {
                md.writes = written;
                info!("re-opening (RW) crate's md {md_path}");
                md.write_to(&md_path)?;
            }

            if let Some(incremental) = incremental {
                if let Err(e) = build(incremental_stage, &incremental).await {
                    warn!("Error building incremental data: {e}");
                }
            }
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

#[derive(Debug)]
struct NamedMount {
    name: Stage,
    src: Utf8PathBuf,
    dst: Utf8PathBuf,
}

fn assemble_build_dependencies(
    md: &mut Md,
    crate_type: &str,
    emit: &str,
    externs: IndexSet<String>,
    target_path: &Utf8Path,
) -> Result<(Vec<NamedMount>, Vec<Md>)> {
    let mut mds = HashMap::<Utf8PathBuf, Md>::new(); // A file cache

    let md_pather = |part: &str| target_path.join(format!("{part}.toml"));

    // https://github.com/rust-lang/cargo/issues/12059#issuecomment-1537457492
    //   https://github.com/rust-lang/rust/issues/63012 : Tracking issue for -Z binary-dep-depinfo
    let mut all_externs = IndexSet::new();

    let ext = match crate_type {
        "lib" => "rmeta".to_owned(),
        "bin" | "rlib" | "test" | "proc-macro" => "rlib".to_owned(),
        _ => bail!("BUG: unexpected crate-type: '{crate_type}'"),
    };
    // https://rustc-dev-guide.rust-lang.org/backend/libs-and-metadata.html#rmeta
    // > [rmeta] is created if the --emit=metadata CLI option is used.
    let ext = if emit.contains("metadata") { "rmeta".to_owned() } else { ext };

    for xtern in externs {
        trace!("❯ extern {xtern}");
        all_externs.insert(xtern.clone());

        if !xtern.starts_with("lib") {
            bail!("BUG: expected extern to match ^lib: {xtern}")
        }
        let xtern = xtern.strip_prefix("lib").expect("PROOF: ~ ^lib");
        let xtern = if xtern.ends_with(".rlib") {
            xtern.strip_suffix(".rlib")
        } else if xtern.ends_with(".rmeta") {
            xtern.strip_suffix(".rmeta")
        } else if xtern.ends_with(".so") {
            xtern.strip_suffix(".so")
        } else {
            bail!("BUG: cargo gave unexpected extern: {xtern:?}")
        }
        .expect("PROOF: all cases match");
        trace!("❯ short extern {xtern}");
        md.short_externs.insert(xtern.to_owned());

        let extern_md = md_pather(xtern);
        info!("checking (RO) extern's externs {extern_md}");
        let extern_md = get_or_read(&mut mds, &extern_md)?;
        for transitive in extern_md.short_externs {
            let guard_md = get_or_read(&mut mds, &md_pather(&transitive))?;
            let ext = if guard_md.is_proc_macro { "so" } else { &ext };

            trace!("❯ extern lib{transitive}.{ext}");
            all_externs.insert(format!("lib{transitive}.{ext}"));

            trace!("❯ short extern {transitive}");
            md.short_externs.insert(transitive);
        }
    }

    let mut mounts = Vec::with_capacity(all_externs.len());
    let extern_mds_and_paths = all_externs
        .into_iter()
        .map(|xtern| {
            let Some((extern_md_path, xtern_stage)) = toml_path_and_stage(&xtern, target_path)
            else {
                bail!("Unexpected extern name format: {xtern}")
            };
            let mount = NamedMount {
                name: xtern_stage,
                src: format!("/{xtern}").into(),
                dst: target_path.join("deps").join(xtern),
            };
            mounts.push(mount);

            let extern_md = get_or_read(&mut mds, &extern_md_path)?;
            Ok((extern_md_path, extern_md))
        })
        .collect::<Result<Vec<_>>>()?;

    let extern_md_paths = md.sort_deps(extern_mds_and_paths)?;
    info!("extern_md_paths: {} {extern_md_paths:?}", extern_md_paths.len());

    let mds = extern_md_paths
        .into_iter()
        .map(|extern_md_path| get_or_read(&mut mds, &extern_md_path))
        .collect::<Result<Vec<_>>>()?;

    Ok((mounts, mds))
}

fn get_or_read(mds: &mut HashMap<Utf8PathBuf, Md>, path: &Utf8Path) -> Result<Md> {
    if let Some(md) = mds.get(path) {
        return Ok(md.clone());
    }
    let md = Md::from_file(path)?;
    let _ = mds.insert(path.to_path_buf(), md.clone());
    Ok(md)
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
            "CARGO_MANIFEST_DIR" | "CARGO_MANIFEST_PATH" => {
                rewrite_cratesio_index(Utf8Path::new(&val)).to_string()
            }
            "TERM" => return None,
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

#[test]
fn toml_path_and_stage_for_rlib() {
    let xtern = "libstrsim-8ed1051e7e58e636.rlib";
    let res = toml_path_and_stage(xtern, "./target/path".into()).unwrap();
    assert_eq!(res.0, "./target/path/strsim-8ed1051e7e58e636.toml".to_owned());
    assert_eq!(res.1, "out-8ed1051e7e58e636".try_into().unwrap());
}

#[test]
fn toml_path_and_stage_for_libc() {
    let xtern = "liblibc-c53783e3f8edcfe4.rmeta";
    let res = toml_path_and_stage(xtern, "./target/path".into()).unwrap();
    assert_eq!(res.0, "./target/path/libc-c53783e3f8edcfe4.toml".to_owned());
    assert_eq!(res.1, "out-c53783e3f8edcfe4".try_into().unwrap());
}

#[test]
fn toml_path_and_stage_for_weird_extension() {
    let xtern = "libthing-131283e3f8edcfe4.a.2.c";
    let res = toml_path_and_stage(xtern, "./target/path".into()).unwrap();
    assert_eq!(res.0, "./target/path/thing-131283e3f8edcfe4.toml".to_owned());
    assert_eq!(res.1, "out-131283e3f8edcfe4".try_into().unwrap());
}

#[must_use]
fn toml_path_and_stage(xtern: &str, target_path: &Utf8Path) -> Option<(Utf8PathBuf, Stage)> {
    // TODO: drop stripping ^lib
    assert!(xtern.starts_with("lib"), "BUG: unexpected xtern format: {xtern}");
    let pa = xtern.strip_prefix("lib").and_then(|x| x.split_once('.')).map(|(x, _)| x);
    let st = pa.and_then(|x| x.split_once('-')).map(|(_, x)| Stage::output(x).unwrap());
    let pa = pa.map(|x| target_path.join(format!("{x}.toml")));
    pa.zip(st)
}

#[test]
fn crate_out_name_for_some_pkg() {
    let crate_out = Utf8Path::new("/home/maison/target/debug/build/quote-adce79444856d618/out");
    let res = crate_out_name(crate_out);
    assert_eq!(res, "crate_out-adce79444856d618".try_into().unwrap());
}

#[must_use]
fn crate_out_name(name: &Utf8Path) -> Stage {
    name.parent()
        .and_then(|x| x.file_name())
        .and_then(|x| x.rsplit_once('-'))
        .map(|(_, x)| Stage::crate_out(x))
        .expect("PROOF: suffix is /out")
        .expect("PROOF: out dir path format")
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
