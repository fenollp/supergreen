use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fs::{self, File},
    future::Future,
    io::{BufRead, BufReader, ErrorKind},
    str::FromStr,
};

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, error, info, trace, warn};
use tokio::process::Command;

use crate::{
    base::RUST,
    checkouts,
    cratesio::{self, rewrite_cratesio_index},
    envs::{self, base_image, internal, maybe_log, pass_env, runner, syntax, this},
    extensions::{Popped, ShowCmd},
    logging::{self, crate_type_for_logging},
    md::{BuildContext, Md},
    runner::{build, MARK_STDERR, MARK_STDOUT},
    rustc_arguments::{as_rustc, RustcArgs},
    stage::Stage,
    PKG, REPO, VSN,
};

// NOTE: this RUSTC_WRAPPER program only ever gets called by `cargo`, so we save
//       ourselves some trouble and assume std::path::{Path, PathBuf} are UTF-8.
//       Or in the words of this crate: https://github.com/camino-rs/camino/tree/8bec62382e1bce1326ee48f6bf93c46e7a4fde0b#:~:text=there%20are%20already%20many%20systems%2C%20such%20as%20cargo%2C%20that%20only%20support%20utf-8%20paths.%20if%20your%20own%20tool%20interacts%20with%20any%20such%20system%2C%20you%20can%20assume%20that%20paths%20are%20valid%20utf-8%20without%20creating%20any%20additional%20burdens%20on%20consumers.

pub(crate) async fn main(
    arg0: Option<String>,
    args: Vec<String>,
    vars: BTreeMap<String, String>,
) -> Result<()> {
    let argz = args.iter().take(3).map(AsRef::as_ref).collect::<Vec<_>>();

    let argv = |times| args.clone().into_iter().skip(times).collect();

    // TODO: find a better heuristic to ensure `rustc` is rustc
    match &argz[..] {
        [rustc, "--crate-name", crate_name, ..] if rustc.ends_with("rustc") =>
             wrap_rustc(crate_name, argv(1), call_rustc(rustc, argv(1))).await,
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

async fn call_rustc(rustc: &str, args: Vec<String>) -> Result<()> {
    // NOTE: not running inside Docker: local install SHOULD match Docker image setup
    // Meaning: it's up to the user to craft their desired $RUSTCBUILDX_BASE_IMAGE
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
    crate_name: &str,
    arguments: Vec<String>,
    fallback: impl Future<Output = Result<()>>,
) -> Result<()> {
    if this() {
        panic!("It's turtles all the way down!")
    }
    env::set_var(internal::RUSTCBUILDX, "1");

    let pwd = env::current_dir().expect("Getting $PWD");
    let pwd: Utf8PathBuf = pwd.try_into().expect("Encoding $PWD in UTF-8");

    let out_dir_var = env::var("OUT_DIR").ok();
    let (st, args) = as_rustc(&pwd, &arguments, out_dir_var.as_deref())?;

    let buildrs = crate_name == "build_script_build";
    // NOTE: krate_name != crate_name: Gets named build_script_build + s/-/_/g + may actually be a different name
    let krate_name = env::var("CARGO_PKG_NAME").ok().unwrap();

    let krate_version = env::var("CARGO_PKG_VERSION").ok().unwrap();

    let krate_manifest_dir = env::var("CARGO_MANIFEST_DIR").ok().unwrap();
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

    logging::setup(&full_krate_id, internal::RUSTCBUILDX_LOG, internal::RUSTCBUILDX_LOG_STYLE);

    info!("{PKG}@{VSN} original args: {arguments:?} pwd={pwd} st={st:?}");

    do_wrap_rustc(
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

#[expect(clippy::too_many_arguments)]
async fn do_wrap_rustc(
    crate_name: &str,
    krate_name: &str,
    krate_version: String,
    krate_manifest_dir: &Utf8Path,
    krate_repository: String,
    buildrs: bool,
    crate_id: String,
    pwd: Utf8PathBuf,
    args: Vec<String>,
    out_dir_var: Option<String>,
    RustcArgs { crate_type, emit, externs, extrafn, incremental, input, out_dir, target_path }: RustcArgs,
    fallback: impl Future<Output = Result<()>>,
) -> Result<()> {
    let debug = maybe_log();

    let incremental = envs::incremental().then_some(incremental).flatten();

    // NOTE: not `out_dir`
    let crate_out = if let Some(crate_out) = out_dir_var {
        if crate_out.ends_with("/out") {
            info!("listing (RO) crate_out contents {crate_out}");
            let listing = fs::read_dir(&crate_out)
                .map_err(|e| anyhow!("Failed reading crate_out dir {crate_out}: {e}"))?;
            let listing =
                listing.map_while(Result::ok).inspect(|x| info!("contains {x:?}")).count();
            // crate_out dir empty => mount can be dropped
            if listing != 0 {
                let ignore = Utf8PathBuf::from(&crate_out).popped(1).join(".dockerignore");
                fs::write(&ignore, "")
                    .map_err(|e| anyhow!("Failed creating crate_out dockerignore {ignore}: {e}"))?;

                Some(crate_out)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    // https://github.com/rust-lang/cargo/issues/12059#issuecomment-1537457492
    //   https://github.com/rust-lang/rust/issues/63012 : Tracking issue for -Z binary-dep-depinfo
    let mut all_externs = BTreeSet::new();
    let externs_prefix = |part: &str| target_path.as_path().join(format!("externs_{part}"));
    let crate_externs = externs_prefix(&format!("{crate_name}{extrafn}"));

    let mut md = Md::new(&extrafn[1..]); // Drops leading dash

    // A woodlegged way of passing around work cargo-green already did
    // TODO: merge both binaries into a single one
    // * so both versions always match
    // * so passing data from cargo-green to wrapper cannot be interrupted/manipulated
    // * so RUSTCBUILDX_ envs turn into only CARGOGREEN_ envs?
    // * so config is driven only by cargo-green
    md.push_block(
        &Stage::try_new(RUST).expect("rust stage"),
        if let Ok(base_block) = env::var("RUSTCBUILDX_BASE_IMAGE_BLOCK_") {
            base_block
        } else {
            base_image().await.block()
        },
    );

    let ext = match crate_type.as_str() {
        "lib" => "rmeta".to_owned(),
        "bin" | "rlib" | "test" | "proc-macro" => "rlib".to_owned(),
        _ => bail!("BUG: unexpected crate-type: '{crate_type}'"),
    };
    // https://rustc-dev-guide.rust-lang.org/backend/libs-and-metadata.html#rmeta
    // > [rmeta] is created if the --emit=metadata CLI option is used.
    let ext = if emit.contains("metadata") { "rmeta".to_owned() } else { ext };

    if crate_type == "proc-macro" {
        // This way crates that depend on this know they must require it as .so
        let guard = format!("{crate_externs}_proc-macro"); // FIXME: store this bit of info in md file
        info!("opening (RW) {guard}");
        fs::write(&guard, "").map_err(|e| anyhow!("Failed to `touch {guard}`: {e}"))?;
    };

    let mut short_externs = BTreeSet::new();
    for xtern in &externs {
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
            bail!("CONTRACT: cargo gave unexpected extern: {xtern:?}")
        }
        .expect("PROOF: all cases match");
        short_externs.insert(xtern.to_owned());

        let xtern_crate_externs = externs_prefix(xtern);
        info!("checking (RO) extern's externs {xtern_crate_externs}");
        if file_exists_and_is_not_empty(&xtern_crate_externs)? {
            info!("opening (RO) crate externs {xtern_crate_externs}");
            let errf = |e| anyhow!("Failed to `cat {xtern_crate_externs}`: {e}");
            let fd = File::open(&xtern_crate_externs).map_err(errf)?;
            for transitive in BufReader::new(fd).lines().map_while(Result::ok) {
                let guard = externs_prefix(&format!("{transitive}_proc-macro"));
                info!("checking (RO) extern's guard {guard}");
                let ext = if file_exists(&guard)? { "so" } else { &ext };
                let actual_extern = format!("lib{transitive}.{ext}");
                all_externs.insert(actual_extern.clone());

                // ^ this algo tried to "keep track" of actual paths to transitive deps artifacts
                //   however some edge cases (at least 1) go through. That fix seems to bust cache on 2nd builds though v

                if debug.is_some() {
                    let deps_dir = target_path.as_path().join("deps");
                    info!("extern crate's extern matches {deps_dir}/lib*.*");
                    let listing = fs::read_dir(&deps_dir)
                        .map_err(|e| anyhow!("Failed reading directory {deps_dir}: {e}"))?
                        .filter_map(Result::ok)
                        .filter_map(|p| {
                            let p = p.path();
                            p.file_name().map(|p| p.to_string_lossy().to_string())
                        })
                        .filter(|p| p.contains(&transitive))
                        .filter(|p| !p.ends_with(&format!("{transitive}.d")))
                        .map(|p| p.to_string())
                        .collect::<Vec<_>>();
                    if listing != vec![actual_extern.clone()] {
                        warn!("instead of [{actual_extern}], listing found {listing:?}");
                    }
                    //all_externs.extend(listing.into_iter());
                    // TODO: move to after for loop
                }

                short_externs.insert(transitive);
            }
        }
    }
    info!("checking (RO) externs {crate_externs}");
    if !file_exists_and_is_not_empty(&crate_externs)? {
        let mut shorts = String::new();
        for short_extern in &short_externs {
            shorts.push_str(&format!("{short_extern}\n"));
        }
        info!("writing (RW) externs to {crate_externs}");
        let errf = |e| anyhow!("Failed creating crate externs {crate_externs}: {e}");
        fs::write(&crate_externs, shorts).map_err(errf)?;
    }
    let all_externs = all_externs;
    info!("crate_externs: {crate_externs}");
    if debug.is_some() {
        match fs::read_to_string(&crate_externs) {
            Ok(data) => data,
            Err(e) => e.to_string(),
        }
        .lines()
        .filter(|x| !x.is_empty())
        .for_each(|line| trace!("❯ {line}"));
    }

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

        (Some((stage, Some(src), dst)), Stage::try_new(format!("dep-{crate_id}"))?)
    } else if !krate_repository.is_empty()
        && krate_manifest_dir.starts_with(cargo_home.join("git/checkouts"))
    {
        // Input is of a git checked out dep

        let (stage, src, dst, block) =
            checkouts::into_stage(krate_manifest_dir, &krate_repository).await?;
        md.push_block(&stage, block);

        (Some((stage, Some(src), dst)), Stage::try_new(format!("dep-{crate_id}"))?)
    } else {
        // Input is local code

        let rustc_stage = if input.is_relative() {
            &input
        } else {
            // e.g. $CARGO_HOME/git/checkouts/rustc-version-rs-de99f49481c38c43/48cf99e/src/lib.rs

            //FIXME: try without thi branch
            input
                .strip_prefix(&pwd)
                .map_err(|e| anyhow!("BUG: unexpected input {input:?} ({e})"))?
        }
        .to_string()
        .replace(['/', '.'], "-");

        let rustc_stage = Stage::try_new(format!("cwd-{crate_id}-{rustc_stage}"))?;
        (None, rustc_stage)
    };
    info!("picked {rustc_stage} for {input}");

    let incremental_stage = Stage::try_new(format!("inc{extrafn}")).unwrap();
    let out_stage = Stage::try_new(format!("out{extrafn}")).unwrap();

    let mut rustc_block = String::new();
    rustc_block.push_str(&format!("FROM {RUST} AS {rustc_stage}\n"));
    rustc_block.push_str(&format!("SHELL {:?}\n", ["/bin/bash", "-eux", "-c"]));
    rustc_block.push_str(&format!("WORKDIR {out_dir}\n"));
    if pwd != out_dir {
        rustc_block.push_str(&format!("WORKDIR {pwd}\n"));
    }

    if let Some(ref incremental) = incremental {
        rustc_block.push_str(&format!("WORKDIR {incremental}\n"));
    }

    let cwd = if let Some((name, src, target)) = input_mount.as_ref() {
        rustc_block.push_str("RUN \\\n");
        let source = src.map(|src| format!(",source={src}")).unwrap_or_default();
        rustc_block.push_str(&format!("  --mount=from={name}{source},target={target} \\\n"));

        None
    } else {
        // NOTE: we don't `rm -rf cwd_root`
        let cwd_root = env::temp_dir().join(format!("{PKG}_{VSN}"));
        fs::create_dir_all(&cwd_root)
            .map_err(|e| anyhow!("Failed `mkdir -p {cwd_root:?}`: {e}"))?;

        let ignore = cwd_root.join(".dockerignore");
        fs::write(&ignore, "")
            .map_err(|e| anyhow!("Failed creating cwd dockerignore {ignore:?}: {e}"))?;

        let cwd_path = cwd_root.join(format!("CWD{extrafn}"));
        let cwd_path: Utf8PathBuf = cwd_path
            .clone()
            .try_into()
            .map_err(|e| anyhow!("cwd's {cwd_path:?} UTF-8 encoding is corrupted: {e}"))?;

        // TODO: --build-arg BUILDKIT_CONTEXT_KEEP_GIT_DIR=0 https://docs.docker.com/engine/reference/builder/#buildkit-built-in-build-args
        //   in Git case: do that ^ IFF remote URL + branch/tag/rev can be decided (a la cratesio optimization)
        // https://docs.docker.com/reference/dockerfile/#add---keep-git-dir

        info!(
            "copying all {}files under {pwd} to {cwd_path}",
            if pwd.join(".git").is_dir() { "git " } else { "" }
        );

        copy_dir_all(&pwd, &cwd_path)?;

        // TODO: --mount=bind each file one by one => drop temp dir ctx (needs [multiple] `mkdir -p`[s] first though)
        // This doesn't work: rustc_block.push_str(&format!("  --mount=from=cwd,target={pwd} \\\n"));
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
        let cwd_stage = Stage::try_new(format!("cwd{extrafn}")).unwrap();

        rustc_block.push_str(&format!("WORKDIR {pwd}\n"));
        rustc_block.push_str(&format!("COPY --from={cwd_stage} / .\n"));
        rustc_block.push_str("RUN \\\n");

        Some((cwd_path, cwd_stage))
    };

    if let Some(ref crate_out) = crate_out {
        let named = crate_out_name(crate_out);
        rustc_block.push_str(&format!("  --mount=from={named},target={crate_out} \\\n"));
    }

    md.contexts = [
        input_mount.and_then(|(name, src, target)| {
            src.is_none().then_some((name.to_string(), target.to_string()))
        }),
        cwd.as_ref().map(|(cwd, cwd_stage)| (cwd_stage.to_string(), cwd.to_string())),
        crate_out.map(|crate_out| (crate_out_name(&crate_out), crate_out)),
    ]
    .into_iter()
    .flatten()
    .map(|(name, uri)| BuildContext { name, uri })
    .inspect(|BuildContext { name, uri }| info!("loading {name:?}: {uri}"))
    .collect();
    info!("loading {} Docker contexts", md.contexts.len());

    debug!("all_externs = {all_externs:?}");
    if externs.len() > all_externs.len() {
        bail!("BUG: (externs, all_externs) = {:?}", (externs.len(), all_externs.len()))
    }

    let mut mounts = Vec::with_capacity(all_externs.len());
    let extern_mds = all_externs
        .into_iter()
        .map(|xtern| {
            let Some((extern_md_path, xtern_stage)) = toml_path_and_stage(&xtern, &target_path)
            else {
                bail!("Unexpected extern name format: {xtern}")
            };
            mounts.push((xtern_stage, format!("/{xtern}"), format!("{target_path}/deps/{xtern}")));

            info!("opening (RO) extern md {extern_md_path}");
            let md_raw = fs::read_to_string(&extern_md_path).map_err(|e| {
                if e.kind() == ErrorKind::NotFound {
                    return anyhow!(
                        r#"
                    Looks like `{PKG}` ran on an unkempt project. That's alright!
                    Let's remove the current target directory (note: $CARGO_TARGET_DIR={target_dir})
                    then run your command again.
                "#,
                        target_dir = env::var("CARGO_TARGET_DIR").unwrap_or("<unset>".to_owned()),
                    );
                }
                anyhow!("Failed reading Md {extern_md_path}: {e}")
            })?;

            let extern_md = Md::from_str(&md_raw)?;
            Ok((extern_md_path, extern_md))
        })
        .collect::<Result<Vec<_>>>()?;
    let extern_md_paths = md.extend_from_externs(extern_mds)?;
    info!("extern_md_paths: {} {extern_md_paths:?}", extern_md_paths.len());

    for (name, source, target) in mounts {
        rustc_block
            .push_str(&format!("  --mount=from={name},target={target},source={source} \\\n"));
    }

    rustc_block.push_str(&format!("    env CARGO={:?} \\\n", "$(which cargo)"));

    for (var, val) in env::vars() {
        let (pass, skip, only_buildrs) = pass_env(&var);
        if pass || (buildrs && only_buildrs) {
            if skip {
                debug!("not forwarding env: {var}={val}");
                continue;
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
                _ => val,
            };
            rustc_block.push_str(&format!("        {var}={val} \\\n"));
        }
    }
    rustc_block.push_str("        RUSTCBUILDX=1 \\\n");

    // TODO: find a way to discover these
    // e.g? https://doc.rust-lang.org/cargo/reference/build-scripts.html#rerun-if-env-changed
    // e.g. https://doc.rust-lang.org/cargo/reference/build-scripts.html#rustc-env
    // but actually no! The cargo directives get emitted when running compiled build script,
    // and this is handled by cargo, outside of the wrapper!
    // => cargo upstream issue "pass env vars read/wrote by build script on call to rustc"
    //   => https://github.com/rust-lang/cargo/issues/14444#issuecomment-2305891696
    for var in ["NTPD_RS_GIT_REV", "NTPD_RS_GIT_DATE", "RING_CORE_PREFIX"] {
        if let Ok(v) = env::var(var) {
            warn!("passing ${var}={v:?} env through");
            rustc_block.push_str(&format!("        {var}={v:?} \\\n"));
        }
    }

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

    let input = rewrite_cratesio_index(input.as_path());
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

    let md = md; // Drop mut

    let mut blocks = String::new();
    let mut visited_cratesio_stages = BTreeSet::new();
    for extern_md_path in extern_md_paths {
        info!("opening (RO) extern's md {extern_md_path}");
        let md_raw = fs::read_to_string(&extern_md_path)
            .map_err(|e| anyhow!("Failed reading Md {extern_md_path}: {e}"))?;
        let extern_md = Md::from_str(&md_raw)?;
        extern_md.append_blocks(&mut blocks, &mut visited_cratesio_stages)?;
        blocks.push('\n');
    }
    md.append_blocks(&mut blocks, &mut visited_cratesio_stages)?;

    {
        let md_path = target_path.as_path().join(format!("{crate_name}{extrafn}.toml"));
        let md_ser = md.to_string_pretty()?;

        info!("opening (RW) crate's md {md_path}");
        // TODO? suggest a `cargo clean` then fail
        if internal::log().map(|x| x == "debug").unwrap_or_default() {
            match fs::read_to_string(&md_path) {
                Ok(existing) => pretty_assertions::assert_eq!(&existing, &md_ser),
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => bail!("Failed reading {md_path}: {e}"),
            }
        }
        fs::write(&md_path, md_ser)
            .map_err(|e| anyhow!("Failed creating crate's md {md_path}: {e}"))?;
    }

    let dockerfile = {
        // TODO: cargo -vv test != cargo test: => the rustc flags will change => Dockerfile needs new cache key
        // => otherwise docker builder cache won't have the correct hit
        // https://rustc-dev-guide.rust-lang.org/backend/libs-and-metadata.html
        //=> a filename suffix with content hash?
        let dockerfile = target_path.as_path().join(format!("{krate_name}{extrafn}.Dockerfile"));

        let syntax = syntax().await.trim_start_matches("docker-image://");
        let mut header = format!("# syntax={syntax}\n");
        header.push_str("# check=error=true\n");
        header.push_str(&format!("# Generated by {REPO} version {VSN}\n"));
        let root = md.rust_stage().expect("Stage 'rust' is the root stage");
        header.push_str(&format!("{}\n", root.script));
        header.push_str(&blocks);

        info!("opening (RW) crate dockerfile {dockerfile}");
        // TODO? suggest a `cargo clean` then fail
        if internal::log().map(|x| x == "debug").unwrap_or_default() {
            match fs::read_to_string(&dockerfile) {
                Ok(existing) => pretty_assertions::assert_eq!(&existing, &header),
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => bail!("Failed reading {dockerfile}: {e}"),
            }
        }

        fs::write(&dockerfile, header)
            .map_err(|e| anyhow!("Failed creating dockerfile {dockerfile}: {e}"))?; // Don't remove this file

        let ignore = format!("{dockerfile}.dockerignore");
        fs::write(&ignore, "")
            .map_err(|e| anyhow!("Failed creating dockerignore {ignore}: {e}"))?;

        if debug.is_some() {
            info!("dockerfile: {dockerfile}");
            match fs::read_to_string(&dockerfile) {
                Ok(data) => data,
                Err(e) => e.to_string(),
            }
            .lines()
            .filter(|x| !x.is_empty())
            .for_each(|line| trace!("❯ {line}"));
        }

        dockerfile
    };

    // TODO: use tracing instead:
    // https://docs.rs/tracing-subscriber/latest/tracing_subscriber/fmt/struct.Subscriber.html
    // https://crates.io/crates/tracing-appender
    // https://github.com/tugglecore/rust-tracing-primer
    // TODO: `cargo green -v{N+1} ..` starts a TUI showing colored logs on above `cargo -v{N} ..`

    let runner = runner();
    if runner == "none" {
        info!("Runner disabled, falling back...");
        return fallback.await;
    }
    let res = build(runner, &dockerfile, out_stage, &md.contexts, &out_dir).await;
    if let Some(incremental) = res.is_ok().then_some(incremental).flatten() {
        let _ = build(runner, &dockerfile, incremental_stage, &md.contexts, &incremental)
            .await
            .inspect_err(|e| warn!("Error fetching incremental data: {e}"));
    }

    if let Err(e) = res {
        warn!("Falling back due to {e}");
        if debug.is_none() {
            // Bubble up actual error & outputs
            return fallback
                .await
                .inspect(|()| eprintln!("BUG: {PKG} should not have encountered this error: {e}"));
        }
        return Err(e);
    }
    Ok(())
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

fn file_exists(path: &Utf8Path) -> Result<bool> {
    match path.metadata().map(|md| md.is_file()) {
        Ok(b) => Ok(b),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(anyhow!("Failed to `stat {path}`: {e}")),
    }
}

pub(crate) fn file_exists_and_is_not_empty(path: &Utf8Path) -> Result<bool> {
    match path.metadata().map(|md| md.is_file() && md.len() > 0) {
        Ok(b) => Ok(b),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(anyhow!("Failed to `test -s {path}`: {e}")),
    }
}

#[test]
fn toml_path_and_stage_for_rlib() {
    let xtern = "libstrsim-8ed1051e7e58e636.rlib";
    let res = toml_path_and_stage(xtern, "./target/path".into()).unwrap();
    assert_eq!(res.0, "./target/path/strsim-8ed1051e7e58e636.toml".to_owned());
    assert_eq!(res.1, "out-8ed1051e7e58e636".to_owned());
}

#[test]
fn toml_path_and_stage_for_libc() {
    let xtern = "liblibc-c53783e3f8edcfe4.rmeta";
    let res = toml_path_and_stage(xtern, "./target/path".into()).unwrap();
    assert_eq!(res.0, "./target/path/libc-c53783e3f8edcfe4.toml".to_owned());
    assert_eq!(res.1, "out-c53783e3f8edcfe4".to_owned());
}

#[test]
fn toml_path_and_stage_for_weird_extension() {
    let xtern = "libthing-131283e3f8edcfe4.a.2.c";
    let res = toml_path_and_stage(xtern, "./target/path".into()).unwrap();
    assert_eq!(res.0, "./target/path/thing-131283e3f8edcfe4.toml".to_owned());
    assert_eq!(res.1, "out-131283e3f8edcfe4".to_owned());
}

#[must_use]
fn toml_path_and_stage(xtern: &str, target_path: &Utf8Path) -> Option<(Utf8PathBuf, String)> {
    // TODO: drop stripping ^lib
    assert!(xtern.starts_with("lib"), "BUG: unexpected xtern format: {xtern}");
    let pa = xtern.strip_prefix("lib").and_then(|x| x.split_once('.')).map(|(x, _)| x);
    let st = pa.and_then(|x| x.split_once('-')).map(|(_, x)| format!("out-{x}"));
    let pa = pa.map(|x| target_path.join(format!("{x}.toml")));
    pa.zip(st)
}

#[test]
fn crate_out_name_for_some_pkg() {
    let crate_out =
        "/home/pete/wefwefwef/buildxargs.git/target/debug/build/quote-adce79444856d618/out";
    let res = crate_out_name(crate_out);
    assert_eq!(res, "crate_out-adce79444856d618".to_owned());
}

#[must_use]
fn crate_out_name(name: &str) -> String {
    Utf8Path::new(name)
        .parent()
        .and_then(|x| x.file_name())
        .and_then(|x| x.rsplit_once('-'))
        .map(|(_, x)| x)
        .map(|x| format!("crate_out-{x}"))
        .expect("PROOF: suffix is /out")
}

fn copy_dir_all(src: &Utf8Path, dst: &Utf8Path) -> Result<()> {
    if dst.exists() {
        return Ok(());
    }

    // Heuristic: test for existence of ./target/CACHEDIR.TAG
    // https://bford.info/cachedir/
    if file_exists_and_is_not_empty(&src.join("CACHEDIR.TAG"))? {
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
            fs::copy(&fpath, dst.join(fname)).map_err(|e| {
                anyhow!("Failed `cp {fpath:?} {dst:?}` ({:?}): {e}", entry.metadata())
            })?;
        }
    }
    Ok(())
}
