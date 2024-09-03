use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    env,
    fs::{self, File},
    future::Future,
    io::{BufRead, BufReader, ErrorKind},
    path::Path,
    process::ExitCode,
    str::FromStr,
};

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use env_logger::{Env, Target};
use tokio::process::Command;

use crate::{
    base::RUST,
    cli::exit_code,
    cratesio::{from_cratesio_input_path, into_stage},
    envs::{self, base_image, internal, maybe_log, pass_env, runner, syntax, this},
    extensions::{Popped, ShowCmd},
    md::{BuildContext, Md},
    parse::{self, RustcArgs},
    runner::{build, MARK_STDERR, MARK_STDOUT},
    stage::Stage,
    PKG, REPO, VSN,
};

const BUILDRS_CRATE_NAME: &str = "build_script_build";

// NOTE: this RUSTC_WRAPPER program only ever gets called by `cargo`, so we save
//       ourselves some trouble and assume std::path::{Path, PathBuf} are UTF-8.
//       Or in the words of this crate: https://github.com/camino-rs/camino/tree/8bec62382e1bce1326ee48f6bf93c46e7a4fde0b#:~:text=there%20are%20already%20many%20systems%2C%20such%20as%20cargo%2C%20that%20only%20support%20utf-8%20paths.%20if%20your%20own%20tool%20interacts%20with%20any%20such%20system%2C%20you%20can%20assume%20that%20paths%20are%20valid%20utf-8%20without%20creating%20any%20additional%20burdens%20on%20consumers.

pub(crate) async fn do_wrap() -> ExitCode {
    let arg0 = env::args().nth(1);
    let args = env::args().skip(1).collect();
    let vars = env::vars().collect();
    match fallible_main(arg0, args, vars).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Wrapped: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn fallible_main(
    arg0: Option<String>,
    args: VecDeque<String>,
    vars: BTreeMap<String, String>,
) -> Result<ExitCode> {
    let argz = args.iter().take(3).map(AsRef::as_ref).collect::<Vec<_>>();

    let argv = |times| {
        let mut argv = args.clone();
        for _ in 0..times {
            argv.pop_front(); // shift 1
        }
        argv.into_iter().collect()
    };

    // TODO: find a better heuristic to ensure `rustc` is rustc
    match &argz[..] {
        [rustc, "--crate-name", crate_name, ..] if rustc.ends_with("rustc") /*&& !envs::incremental()*/ =>
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

async fn call_rustc(rustc: &str, args: Vec<String>) -> Result<ExitCode> {
    // NOTE: not running inside Docker: local install SHOULD match Docker image setup
    // Meaning: it's up to the user to craft their desired $RUSTCBUILDX_BASE_IMAGE
    let mut cmd = Command::new(rustc);
    let cmd = cmd.kill_on_drop(true).args(args);
    let code = cmd
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn {}: {e}", cmd.show()))?
        .wait()
        .await
        .map_err(|e| anyhow!("Failed to wait {}: {e}", cmd.show()))?
        .code();
    Ok(exit_code(code))
}

async fn wrap_rustc(
    crate_name: &str,
    arguments: Vec<String>,
    fallback: impl Future<Output = Result<ExitCode>>,
) -> Result<ExitCode> {
    if this() {
        panic!("It's turtles all the way down!")
    }
    env::set_var(internal::RUSTCBUILDX, "1");

    if let Some(log_file) = maybe_log() {
        env_logger::Builder::from_env(
            Env::default()
                .filter_or(internal::RUSTCBUILDX_LOG, "debug")
                .write_style(internal::RUSTCBUILDX_LOG_STYLE),
        )
        .target(Target::Pipe(Box::new(log_file().expect("Installing logfile"))))
        .init();
    }

    let pwd = env::current_dir().expect("Getting $PWD");
    let pwd: Utf8PathBuf = pwd.try_into().expect("Encoding $PWD in UTF-8");

    let krate = format!("{PKG}:{crate_name}");
    log::info!(target: &krate, "{PKG}@{VSN} original args: {arguments:?} pwd={pwd}");

    do_wrap_rustc(crate_name, arguments, fallback, &krate, pwd)
        .await
        .inspect_err(|e| log::error!(target: &krate, "Error: {e}"))
}

async fn do_wrap_rustc(
    crate_name: &str,
    arguments: Vec<String>,
    fallback: impl Future<Output = Result<ExitCode>>,
    krate: &str,
    pwd: Utf8PathBuf,
) -> Result<ExitCode> {
    let debug = maybe_log();

    let out_dir_var = env::var("OUT_DIR").ok();
    let (st, args) = parse::as_rustc(&pwd, arguments, out_dir_var.as_deref())?;
    log::info!(target: &krate, "{st:?}");
    let RustcArgs { crate_type, emit, externs, metadata, incremental, input, out_dir, target_path } =
        st;
    let incremental = envs::incremental().then_some(incremental).flatten();

    let full_krate_id = {
        let krate_version = env::var("CARGO_PKG_VERSION").ok().unwrap_or_default();
        let krate_name = env::var("CARGO_PKG_NAME").ok().unwrap_or_default();
        if crate_name == BUILDRS_CRATE_NAME {
            if crate_type != "bin" {
                bail!("BUG: expected build script to be of crate_type bin, got: {crate_type}")
            }
            format!("buildrs|{krate_name}|{krate_version}|{metadata}")
        } else {
            format!("{crate_type}|{krate_name}|{krate_version}|{metadata}")
        }
    };
    let krate = full_krate_id.as_str();
    let crate_id = full_krate_id.replace('|', "-");

    // NOTE: not `out_dir`
    let crate_out = if let Some(crate_out) = out_dir_var {
        if crate_out.ends_with("/out") {
            log::info!(target: &krate, "listing (RO) crate_out contents {crate_out}");
            let listing = fs::read_dir(&crate_out)
                .map_err(|e| anyhow!("Failed reading crate_out dir {crate_out}: {e}"))?;
            let listing = listing
                .map_while(Result::ok)
                .inspect(|x| log::info!(target: &krate, "contains {x:?}"))
                .count();
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
    let externs_prefix = |part: &str| Utf8Path::new(&target_path).join(format!("externs_{part}"));
    let crate_externs = externs_prefix(&format!("{crate_name}-{metadata}"));

    let mut md = Md::new(&metadata);

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
        log::info!(target: &krate, "opening (RW) {guard}");
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
        log::info!(target: &krate, "checking (RO) extern's externs {xtern_crate_externs}");
        if file_exists_and_is_not_empty(&xtern_crate_externs)? {
            log::info!(target: &krate, "opening (RO) crate externs {xtern_crate_externs}");
            let errf = |e| anyhow!("Failed to `cat {xtern_crate_externs}`: {e}");
            let fd = File::open(&xtern_crate_externs).map_err(errf)?;
            for transitive in BufReader::new(fd).lines().map_while(Result::ok) {
                let guard = externs_prefix(&format!("{transitive}_proc-macro"));
                log::info!(target: &krate, "checking (RO) extern's guard {guard}");
                let ext = if file_exists(&guard)? { "so" } else { &ext };
                let actual_extern = format!("lib{transitive}.{ext}");
                all_externs.insert(actual_extern.clone());

                // ^ this algo tried to "keep track" of actual paths to transitive deps artifacts
                //   however some edge cases (at least 1) go through. That fix seems to bust cache on 2nd builds though v

                if debug.is_some() {
                    let deps_dir = Utf8Path::new(&target_path).join("deps");
                    log::info!(target: &krate, "extern crate's extern matches {deps_dir}/lib*.*");
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
                        log::warn!(target: &krate,"instead of [{actual_extern}], listing found {listing:?}");
                    }
                    //all_externs.extend(listing.into_iter());
                    // TODO: move to after for loop
                }

                short_externs.insert(transitive);
            }
        }
    }
    log::info!(target: &krate, "checking (RO) externs {crate_externs}");
    if !file_exists_and_is_not_empty(&crate_externs)? {
        let mut shorts = String::new();
        for short_extern in &short_externs {
            shorts.push_str(&format!("{short_extern}\n"));
        }
        log::info!(target: &krate, "writing (RW) externs to {crate_externs}");
        let errf = |e| anyhow!("Failed creating crate externs {crate_externs}: {e}");
        fs::write(&crate_externs, shorts).map_err(errf)?;
    }
    let all_externs = all_externs;
    log::info!(target: &krate, "crate_externs: {crate_externs}");
    if debug.is_some() {
        match fs::read_to_string(&crate_externs) {
            Ok(data) => data,
            Err(e) => e.to_string(),
        }
        .lines()
        .filter(|x| !x.is_empty())
        .for_each(|line| log::debug!(target: &krate, "❯ {line}"));
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

    // TODO: allow opt-out of cratesio_stage => to support offline builds
    // TODO: or, allow a `cargo fetch` alike: create+pre-build all cratesio stages from lockfile
    let (input_mount, rustc_stage) = if input.starts_with(cargo_home.join("registry/src")) {
        // Input is of a crate dep (hosted at crates.io)
        // Let's optimize this case by fetching & caching crate tarball

        let (name, version, cratesio_index) = from_cratesio_input_path(&input)?;
        let (cratesio_stage, src, dst, block) =
            into_stage(krate, cargo_home.as_path(), &name, &version, &cratesio_index).await?;
        md.push_block(&cratesio_stage, block);

        let rustc_stage = Stage::try_new(format!("dep-{crate_id}-{cratesio_index}"))?;
        (Some((cratesio_stage, Some(src), dst)), rustc_stage)
    } else {
        // Input is local code

        let rustc_stage = if input.is_relative() {
            &input
        } else {
            // e.g. $CARGO_HOME/git/checkouts/rustc-version-rs-de99f49481c38c43/48cf99e/src/lib.rs
            // TODO: create a stage from sources where able (public repos) use --secret mounts for private deps (and secret direct artifacts)
            // TODO=> make sense of git origin file://$CARGO_HOME/git/db/rustc-version-rs-de99f49481c38c43

            // env is set: CARGO_MANIFEST_DIR="$CARGO_HOME/git/checkouts/rustc-version-rs-de99f49481c38c43/48cf99e"
            // => commit
            // env is set: CARGO_PKG_REPOSITORY="https://github.com/djc/rustc-version-rs"
            // => url
            // copying all git files under $CARGO_HOME/git/checkouts/rustc-version-rs-de99f49481c38c43/48cf99e to /tmp/cargo-green_0.7.0/CWD2ee709abf3a7f0b4
            // loading "cwd-2ee709abf3a7f0b4": /tmp/cargo-green_0.7.0/CWD2ee709abf3a7f0b4

            input
                .strip_prefix(&pwd)
                .map_err(|e| anyhow!("BUG: unexpected input {input:?} ({e})"))?
        }
        .to_string()
        .replace(['/', '.'], "-");

        let rustc_stage = Stage::try_new(format!("cwd-{crate_id}-{rustc_stage}"))?;
        (None, rustc_stage)
    };
    log::info!(target: &krate, "picked {rustc_stage} for {input}");

    let incremental_stage = Stage::try_new(format!("inc-{metadata}"))?;
    let out_stage = Stage::try_new(format!("out-{metadata}"))?;

    let mut rustc_block = String::new();
    rustc_block.push_str(&format!("FROM {RUST} AS {rustc_stage}\n"));
    rustc_block.push_str(&format!("SHELL {:?}\n", ["/bin/bash", "-eux", "-c"]));
    rustc_block.push_str(&format!("WORKDIR {out_dir}\n"));

    if let Some(ref incremental) = incremental {
        rustc_block.push_str(&format!("WORKDIR {incremental}\n"));
    }

    rustc_block.push_str("RUN \\\n");
    let cwd = if let Some((name, src, target)) = input_mount.as_ref() {
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

        let cwd = cwd_root.join(format!("CWD{metadata}"));
        let cwd_path: Utf8PathBuf = cwd
            .clone()
            .try_into()
            .map_err(|e| anyhow!("cwd's {cwd:?} UTF-8 encoding is corrupted: {e}"))?;

        // TODO: IFF remote URL + branch/tag/rev can be decided (a la cratesio optimization)
        // https://docs.docker.com/reference/dockerfile/#add---keep-git-dir

        log::info!(target: &krate, "copying all {}files under {pwd} to {cwd_path}", if pwd.join(".git").is_dir() { "git " } else { "" });

        // https://docs.rs/ignore/latest/ignore/struct.WalkBuilder.html

        // https://docs.docker.com/engine/storage/bind-mounts/
        // https://docs.docker.com/reference/dockerfile/#run---mount

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

        // TODO? skip copy: mount pwd

        // [2024-08-25T18:49:07Z INFO  lib|buildxargs|1.2.0|a3502a85761ca193] copying all git files under /home/pete/.cargo/git/checkouts/buildxargs-c31ec630f7b14578/c968ea3 to /tmp/cargo-green_0.8.0/CWDa3502a85761ca193
        // >>> cp "/home/pete/.cargo/git/checkouts/buildxargs-c31ec630f7b14578/c968ea3/.cargo-ok" "/tmp/cargo-green_0.8.0/CWDa3502a85761ca193/.cargo-ok"
        // >>> cp "/home/pete/.cargo/git/checkouts/buildxargs-c31ec630f7b14578/c968ea3/.github/dependabot.yml" "/tmp/cargo-green_0.8.0/CWDa3502a85761ca193/.github/dependabot.yml"
        // >>> cp "/home/pete/.cargo/git/checkouts/buildxargs-c31ec630f7b14578/c968ea3/.github/workflows/rust.yml" "/tmp/cargo-green_0.8.0/CWDa3502a85761ca193/.github/workflows/rust.yml"
        // >>> cp "/home/pete/.cargo/git/checkouts/buildxargs-c31ec630f7b14578/c968ea3/README.md" "/tmp/cargo-green_0.8.0/CWDa3502a85761ca193/README.md"
        // >>> cp "/home/pete/.cargo/git/checkouts/buildxargs-c31ec630f7b14578/c968ea3/Cargo.lock" "/tmp/cargo-green_0.8.0/CWDa3502a85761ca193/Cargo.lock"
        // >>> cp "/home/pete/.cargo/git/checkouts/buildxargs-c31ec630f7b14578/c968ea3/rustfmt.toml" "/tmp/cargo-green_0.8.0/CWDa3502a85761ca193/rustfmt.toml"
        // >>> cp "/home/pete/.cargo/git/checkouts/buildxargs-c31ec630f7b14578/c968ea3/src/main.rs" "/tmp/cargo-green_0.8.0/CWDa3502a85761ca193/src/main.rs"
        // >>> cp "/home/pete/.cargo/git/checkouts/buildxargs-c31ec630f7b14578/c968ea3/src/lib.rs" "/tmp/cargo-green_0.8.0/CWDa3502a85761ca193/src/lib.rs"
        // >>> cp "/home/pete/.cargo/git/checkouts/buildxargs-c31ec630f7b14578/c968ea3/.gitignore" "/tmp/cargo-green_0.8.0/CWDa3502a85761ca193/.gitignore"
        // >>> cp "/home/pete/.cargo/git/checkouts/buildxargs-c31ec630f7b14578/c968ea3/Cargo.toml" "/tmp/cargo-green_0.8.0/CWDa3502a85761ca193/Cargo.toml"
        // >>> cp "/home/pete/.cargo/git/checkouts/buildxargs-c31ec630f7b14578/c968ea3/LICENSE" "/tmp/cargo-green_0.8.0/CWDa3502a85761ca193/LICENSE"
        // >>> cp "/home/pete/.cargo/git/checkouts/buildxargs-c31ec630f7b14578/c968ea3/tests/lib.rs" "/tmp/cargo-green_0.8.0/CWDa3502a85761ca193/tests/lib.rs"
        // >>> cp "/home/pete/.cargo/git/checkouts/buildxargs-c31ec630f7b14578/c968ea3/tests/buildx.rs" "/tmp/cargo-green_0.8.0/CWDa3502a85761ca193/tests/buildx.rs"
        // >>> cp "/home/pete/.cargo/git/checkouts/buildxargs-c31ec630f7b14578/c968ea3/tests/usage.rs" "/tmp/cargo-green_0.8.0/CWDa3502a85761ca193/tests/usage.rs"
        // >>> cp "/home/pete/.cargo/git/checkouts/buildxargs-c31ec630f7b14578/c968ea3/tests/cli.rs" "/tmp/cargo-green_0.8.0/CWDa3502a85761ca193/tests/cli.rs"
        // [2024-08-25T18:49:07Z INFO  lib|buildxargs|1.2.0|a3502a85761ca193] loading 1 Docker contexts
        // [2024-08-25T18:49:07Z INFO  lib|buildxargs|1.2.0|a3502a85761ca193] loading "cwd-a3502a85761ca193": /tmp/cargo-green_0.8.0/CWDa3502a85761ca193

        // >>> cp "/home/pete/wefwefwef/supergreen.git/hack/clis.sh" "/tmp/cargo-green_0.8.0/CWDf273b3fc9f002200/hack/clis.sh"
        // >>> cp "/home/pete/wefwefwef/supergreen.git/README.md" "/tmp/cargo-green_0.8.0/CWDf273b3fc9f002200/README.md"
        // >>> cp "/home/pete/wefwefwef/supergreen.git/Cargo.lock" "/tmp/cargo-green_0.8.0/CWDf273b3fc9f002200/Cargo.lock"
        // >>> cp "/home/pete/wefwefwef/supergreen.git/rustfmt.toml" "/tmp/cargo-green_0.8.0/CWDf273b3fc9f002200/rustfmt.toml"
        // >>> cp "/home/pete/wefwefwef/supergreen.git/.gitignore" "/tmp/cargo-green_0.8.0/CWDf273b3fc9f002200/.gitignore"
        // >>> cp "/home/pete/wefwefwef/supergreen.git/Cargo.toml" "/tmp/cargo-green_0.8.0/CWDf273b3fc9f002200/Cargo.toml"
        // >>> cp "/home/pete/wefwefwef/supergreen.git/LICENSE" "/tmp/cargo-green_0.8.0/CWDf273b3fc9f002200/LICENSE"
        // [2024-08-25T18:55:03Z INFO  test|cargo-green|0.8.0|f273b3fc9f002200] loading 1 Docker contexts
        // [2024-08-25T18:55:03Z INFO  test|cargo-green|0.8.0|f273b3fc9f002200] loading "cwd-f273b3fc9f002200": /tmp/cargo-green_0.8.0/CWDf273b3fc9f002200
        //
        // ❯ COPY --from=cwd-test-cargo-green-0.8.0-f273b3fc9f002200-cargo-green-src-main-rs /home/pete/wefwefwef/supergreen.git/target/tmp-build/debug/deps/*-f273b3fc9f002200* /
        // out_dir=> /home/pete/wefwefwef/supergreen.git/target/tmp-build/debug/deps

        // /home/pete/wefwefwef/supergreen.git/target/tmp-test/debug/deps  out_dir
        // /home/pete/wefwefwef/supergreen.git                             pwd

        let wef = |pwd: Utf8PathBuf, mut out_dir: Utf8PathBuf| {
            assert_ne!(pwd, Utf8PathBuf::from("/"));
            if !out_dir.starts_with(&pwd) {
                panic!("{out_dir:?}.starts_with(&{pwd:?})")
            }

            loop {
                let parent = out_dir.parent().expect("pwd!=/ && out_dir>pwd");
                if parent == pwd {
                    return out_dir;
                }
                out_dir = parent.to_owned();
            }

            // let mut target = Utf8PathBuf::new();
            // loop {
            //     if out_dir == pwd {
            //         return pwd.join(target);
            //     }
            //     target = out_dir.parent();
            //     let did = out_dir.pop();
            //     assert!(did);
            // }
            // unreachable!("We asserted out_dir.starts_with(pwd)")
        };

        {
            let out_dir =
                Utf8PathBuf::from("/home/pete/wefwefwef/supergreen.git/target/tmp-test/debug/deps");
            let pwd = Utf8PathBuf::from("/home/pete/wefwefwef/supergreen.git");
            assert_eq!(
                wef(pwd, out_dir),
                Utf8PathBuf::from("/home/pete/wefwefwef/supergreen.git/target")
            );
        }

        let target_root_dir = out_dir.starts_with(&pwd).then(|| wef(pwd.clone(), out_dir.clone()));

        log::info!(target: &krate, "blblblbl out_dir {out_dir:?}");
        log::info!(target: &krate, "blblblbl pwd {pwd:?}");
        log::info!(target: &krate, "blblblbl target_root_dir {target_root_dir:?}");

        let cwd_path = cwd_path.to_owned();
        copy_dir_all(&pwd, &cwd_path, target_root_dir.as_ref())?;

        let listing = fs::read_dir(&cwd_path)
            .map(|it| it.inspect(|x| log::info!(target: &krate, "blblblbl contents {x:?}")))
            .map(|it| it.count());
        log::info!(target: &krate, "blblblbl contents {listing:?} {cwd_path:?} (not: {target_root_dir:?})");

        // TODO: do better to avoid copying >1 times local work dir on each cargo call => context-mount local content-addressed tarball?
        // test|cargo-green|0.8.0|f273b3fc9f002200] copying all git files under /home/pete/wefwefwef/supergreen.git to /tmp/cargo-green_0.8.0/CWDf273b3fc9f002200
        // bin|cargo-green|0.8.0|efe5575298075b07] copying all git files under /home/pete/wefwefwef/supergreen.git to /tmp/cargo-green_0.8.0/CWDefe5575298075b07
        let cwd_stage = Stage::try_new(format!("cwd-{metadata}")).expect("empty metadata?");

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
    .collect();
    log::info!(target: &krate, "loading {} Docker contexts", md.contexts.len());
    for BuildContext { name, uri } in &md.contexts {
        log::info!(target: &krate, "loading {name:?}: {uri}");
    }

    log::debug!(target: &krate, "all_externs = {all_externs:?}");
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

            log::info!(target: &krate, "opening (RO) extern md {extern_md_path}");
            let md_raw = fs::read_to_string(&extern_md_path).map_err(|e| {
                if e.kind() == ErrorKind::NotFound {
                    return anyhow!(
                        r#"
                    Looks like `{PKG}` ran on an unkempt project. That's alright!
                    Let's remove the current target directory (note: $CARGO_TARGET_DIR={target_dir})
                    then run your command again.
                "#,
                        target_dir =
                            env::var("CARGO_TARGET_DIR").expect("cargo sets $CARGO_TARGET_DIR"),
                    );
                }
                anyhow!("Failed reading Md {extern_md_path}: {e}")
            })?;

            let extern_md = Md::from_str(&md_raw)?;
            Ok((extern_md_path, extern_md))
        })
        .collect::<Result<Vec<_>>>()?;
    let extern_md_paths = md.extend_from_externs(extern_mds)?;
    log::info!(target: &krate, "extern_md_paths: {} {extern_md_paths:?}", extern_md_paths.len());

    for (name, source, target) in mounts {
        rustc_block
            .push_str(&format!("  --mount=from={name},target={target},source={source} \\\n"));
    }

    if let Some((cwd_path, cwd_stage)) = cwd {
        // Dirty cargo-green v0.8.0 (/home/runner/work/supergreen/supergreen/cargo-green): stale, unknown reason
        // https://github.com/rust-lang/cargo/blob/b57cdb53fba7e8ee3e905c17fc262610ba002474/src/cargo/core/compiler/fingerprint/dirty_reason.rs#L216

        // let listing = fs::read_dir(&pwd)
        //     .map(|it| it.inspect(|x| log::info!(target: &krate, "blblblbl contains {x:?}")))
        //     .map(|it| it.count());
        // log::info!(target: &krate, "blblblbl {listing:?} {pwd:?}");

        // let listing = fs::read_dir(format!("{pwd}/cargo-green"))
        //     .map(|it| it.inspect(|x| log::info!(target: &krate, "blblblbl cargo-green {x:?}")))
        //     .map(|it| it.count());
        // log::info!(target: &krate, "blblblbl {listing:?} {:?}",format!("{pwd}/cargo-green"));

        //bin|cargo-green|0.8.0|efe5575298075b07] ❯ FROM rust-base AS cwd-bin-cargo-green-0.8.0-efe5575298075b07-cargo-green-src-main-rs
        //bin|cargo-green|0.8.0|efe5575298075b07] ❯ WORKDIR /home/runner/work/supergreen/supergreen/target/debug/deps
        //bin|cargo-green|0.8.0|efe5575298075b07] ❯   --mount=type=bind,from=out-caf9440bdb3aa7ed,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libwebpki-caf9440bdb3aa7ed.rlib,source=/libwebpki-caf9440bdb3aa7ed.rlib,ro \
        //bin|cargo-green|0.8.0|efe5575298075b07] ❯   --mount=type=bind,from=out-05c6048285b4fe4e,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libwebpki_roots-05c6048285b4fe4e.rlib,source=/libwebpki_roots-05c6048285b4fe4e.rlib,ro \
        //bin|cargo-green|0.8.0|efe5575298075b07] ❯   --mount=type=bind,from=out-122df945fd947c3b,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libwinnow-122df945fd947c3b.rlib,source=/libwinnow-122df945fd947c3b.rlib,ro \
        //bin|cargo-green|0.8.0|efe5575298075b07] ❯   --mount=type=bind,from=out-d6dc2b3b3f64b3a5,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libyansi-d6dc2b3b3f64b3a5.rlib,source=/libyansi-d6dc2b3b3f64b3a5.rlib,ro \
        //bin|cargo-green|0.8.0|efe5575298075b07] ❯   --mount=type=bind,from=out-a3c891722edb5521,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libzeroize-a3c891722edb5521.rlib,source=/libzeroize-a3c891722edb5521.rlib,ro \
        //bin|cargo-green|0.8.0|efe5575298075b07] ❯   --mount=type=bind,from=cwd-efe5575298075b07,target=/home/runner/work/supergreen/supergreen,rw cd /home/runner/work/supergreen/supergreen && \
        //bin|cargo-green|0.8.0|efe5575298075b07] ❯     set -eux \
        //bin|cargo-green|0.8.0|efe5575298075b07] ❯  && export CARGO="$(which cargo)" \
        //bin|cargo-green|0.8.0|efe5575298075b07] ❯  && /bin/bash -c "rustc '--crate-name' 'cargo_green' '--edition' '2021' '--error-format' 'json' '--json' 'diagnostic-rendered-ansi,artifacts,future-incompat' '--crate-type' 'bin' '--emit' 'dep-info,link' '-C' 'embed-bitcode=no' '-C' 'debuginfo=2' '--check-cfg' 'cfg(docsrs)' '--check-cfg' 'cfg(feature, values())' '-C' 'metadata=efe5575298075b07' '-C' 'extra-filename=-efe5575298075b07' '--out-dir' '/home/runner/work/supergreen/supergreen/target/debug/deps' '-C' 'incremental=/home/runner/work/supergreen/supergreen/target/debug/incremental' '-L' 'dependency=/home/runner/work/supergreen/supergreen/target/debug/deps' '--extern' 'anyhow=/home/runner/work/supergreen/supergreen/target/debug/deps/libanyhow-8994ddf264b506dd.rlib' '--extern' 'camino=/home/runner/work/supergreen/supergreen/target/debug/deps/libcamino-7471cc044e43fae8.rlib' '--extern' 'env_logger=/home/runner/work/supergreen/supergreen/target/debug/deps/libenv_logger-8bf0bc253e2b5e56.rlib' '--extern' 'futures=/home/runner/work/supergreen/supergreen/target/debug/deps/libfutures-7d5afd56343d7d1b.rlib' '--extern' 'home=/home/runner/work/supergreen/supergreen/target/debug/deps/libhome-553e02f09d173e01.rlib' '--extern' 'log=/home/runner/work/supergreen/supergreen/target/debug/deps/liblog-6865073cb12eb520.rlib' '--extern' 'nutype=/home/runner/work/supergreen/supergreen/target/debug/deps/libnutype-2d0a379f7162102a.rlib' '--extern' 'pretty_assertions=/home/runner/work/supergreen/supergreen/target/debug/deps/libpretty_assertions-8016036a38e95dc8.rlib' '--extern' 'reqwest=/home/runner/work/supergreen/supergreen/target/debug/deps/libreqwest-d912a0b62e27096a.rlib' '--extern' 'rustc_version=/home/runner/work/supergreen/supergreen/target/debug/deps/librustc_version-02ab48bfebb94691.rlib' '--extern' 'rustflags=/home/runner/work/supergreen/supergreen/target/debug/deps/librustflags-a52071a8332cf8cd.rlib' '--extern' 'serde=/home/runner/work/supergreen/supergreen/target/debug/deps/libserde-e8d25659f6b92157.rlib' '--extern' 'serde_jsonlines=/home/runner/work/supergreen/supergreen/target/debug/deps/libserde_jsonlines-b9abadeabde9e733.rlib' '--extern' 'serde_json=/home/runner/work/supergreen/supergreen/target/debug/deps/libserde_json-363ab0e04bb26d42.rlib' '--extern' 'sha256=/home/runner/work/supergreen/supergreen/target/debug/deps/libsha256-fa64a708960a507e.rlib' '--extern' 'szyk=/home/runner/work/supergreen/supergreen/target/debug/deps/libszyk-15383af1e4a4ba93.rlib' '--extern' 'tokio=/home/runner/work/supergreen/supergreen/target/debug/deps/libtokio-5bf6c2b013a5c1c7.rlib' '--extern' 'toml=/home/runner/work/supergreen/supergreen/target/debug/deps/libtoml-5724af712b40fb32.rlib' '-L' 'native=/home/runner/work/supergreen/supergreen/target/debug/build/ring-eff462b40b2a19a0/out' cargo-green/src/main.rs \
        //bin|cargo-green|0.8.0|efe5575298075b07] ❯       1> >(sed 's/^/::STDOUT:: /') \
        //bin|cargo-green|0.8.0|efe5575298075b07] ❯       2> >(sed 's/^/::STDERR:: /' >&2)"
        //bin|cargo-green|0.8.0|efe5575298075b07] ❯ FROM scratch AS out-efe5575298075b07
        //bin|cargo-green|0.8.0|efe5575298075b07] ❯ COPY --from=cwd-bin-cargo-green-0.8.0-efe5575298075b07-cargo-green-src-main-rs /home/runner/work/supergreen/supergreen/target/debug/deps/*-efe5575298075b07* /
        //bin|cargo-green|0.8.0|efe5575298075b07] Starting `docker --debug build --network=none --platform=local --pull=false --target=out-efe5575298075b07 --output=type=local,dest=/home/runner/work/supergreen/supergreen/target/debug/deps --build-context=crate_out-7b1ca16f88880f2c=/home/runner/work/supergreen/supergreen/target/debug/build/proc-macro2-7b1ca16f88880f2c/out --build-context=crate_out-a95bf2cd57287be3=/home/runner/work/supergreen/supergreen/target/debug/build/typenum-a95bf2cd57287be3/out --build-context=crate_out-c1196db1e19f7e7b=/home/runner/work/supergreen/supergreen/target/debug/build/anyhow-c1196db1e19f7e7b/out --build-context=crate_out-eff462b40b2a19a0=/home/runner/work/supergreen/supergreen/target/debug/build/ring-eff462b40b2a19a0/out --build-context=cwd-efe5575298075b07=/tmp/cargo-green_0.8.0/CWDefe5575298075b07 --file=/home/runner/work/supergreen/supergreen/target/debug/cargo_green-efe5575298075b07.Dockerfile /home/runner/work/supergreen/supergreen/target/debug` (env: "\"DOCKER_BUILDKIT\"=Some(\"1\")")`

        rustc_block.push_str(&format!("  --mount=from={cwd_stage},target={pwd},rw \\\n"));
        // rustc_block.push_str(&format!(
        //     " ls -lha {pwd} {pwd}/target {pwd}/target/debug {pwd}/target/debug/dqeps ; exit 42 \\\n"
        // ));

        //v

        rustc_block.push_str(&format!("    cd {pwd} \\\n"));

        // rustc_block
        //     .push_str(&format!("    cd {pwd} && ls -lha . && sleep 2 && if [ -d ./target ]; then echo blblblbl && exit 42; fi \\\n"));

        //so we mount things to ./target
        //then we mount cwd,rw (does not contain ./target)
        //=> still dirty

        //^

        let listing = fs::read_dir(&cwd_path)
            .map(|it| it.inspect(|x| log::info!(target: &krate, "blblblbl contains {x:?}")))
            .map(|it| it.count());
        log::info!(target: &krate, "blblblbl {listing:?} {cwd_path:?}");

        // rustc_block.push_str(&format!("  --mount=from={cwd_stage},target=/tmp/{cwd_stage} \\\n"));
        // rustc_block.push_str("    set -eux \\\n");
        // rustc_block.push_str(&format!(" && mkdir -p {pwd} \\\n"));
        // rustc_block.push_str(&format!(" && cp -pr /tmp/{cwd_stage}/* {pwd}/ \\\n"));
        // rustc_block.push_str(&format!(" && cd {pwd} \\\n"));
        //test|cargo-green|0.8.0|f273b3fc9f002200@34172] ✖ #652 0.467 cp: cannot create regular file '/home/runner/work/supergreen/supergreen/target/debug/deps/libyansi-d6dc2b3b3f64b3a5.rlib': Read-only file system
        //test|cargo-green|0.8.0|f273b3fc9f002200@34172] ✖ #652 0.467 cp: cannot create regular file '/home/runner/work/supergreen/supergreen/target/debug/deps/libzeroize-a3c891722edb5521.rlib': Read-only file system
        //test|cargo-green|0.8.0|f273b3fc9f002200@34172] ✖ #652 ERROR: process "/bin/sh -c set -eux  && mkdir -p /home/runner/work/supergreen/supergreen  && cp -pr /tmp/cwd-f273b3fc9f002200/* /home/runner/work/supergreen/supergreen/  && cd /home/runner/work/supergreen/supergreen  && export CARGO=\"$(which cargo)\"  && /bin/bash -c \"rustc '--crate-name' 'cargo_green' '--edition' '2021' '--error-format' 'json' '--json' 'diagnostic-rendered-ansi,artifacts,future-incompat' '--emit' 'dep-info,link' '-C' 'embed-bitcode=no' '-C' 'debuginfo=2' '--test' '--check-cfg' 'cfg(docsrs)' '--check-cfg' 'cfg(feature, values())' '-C' 'metadata=f273b3fc9f002200' '-C' 'extra-filename=-f273b3fc9f002200' '--out-dir' '/home/runner/work/supergreen/supergreen/target/debug/deps' '-C' 'incremental=/home/runner/work/supergreen/supergreen/target/debug/incremental' '-L' 'dependency=/home/runner/work/supergreen/supergreen/target/debug/deps' '--extern' 'anyhow=/home/runner/work/supergreen/supergreen/target/debug/deps/libanyhow-8994ddf264b506dd.rlib' '--extern' 'assertx=/home/runner/work/supergreen/supergreen/target/debug/deps/libassertx-868dc96d8980862a.rlib' '--extern' 'camino=/home/runner/work/supergreen/supergreen/target/debug/deps/libcamino-7471cc044e43fae8.rlib' '--extern' 'env_logger=/home/runner/work/supergreen/supergreen/target/debug/deps/libenv_logger-8bf0bc253e2b5e56.rlib' '--extern' 'futures=/home/runner/work/supergreen/supergreen/target/debug/deps/libfutures-7d5afd56343d7d1b.rlib' '--extern' 'home=/home/runner/work/supergreen/supergreen/target/debug/deps/libhome-553e02f09d173e01.rlib' '--extern' 'log=/home/runner/work/supergreen/supergreen/target/debug/deps/liblog-6865073cb12eb520.rlib' '--extern' 'nutype=/home/runner/work/supergreen/supergreen/target/debug/deps/libnutype-2d0a379f7162102a.rlib' '--extern' 'pretty_assertions=/home/runner/work/supergreen/supergreen/target/debug/deps/libpretty_assertions-8016036a38e95dc8.rlib' '--extern' 'reqwest=/home/runner/work/supergreen/supergreen/target/debug/deps/libreqwest-d912a0b62e27096a.rlib' '--extern' 'rustc_version=/home/runner/work/supergreen/supergreen/target/debug/deps/librustc_version-02ab48bfebb94691.rlib' '--extern' 'rustflags=/home/runner/work/supergreen/supergreen/target/debug/deps/librustflags-a52071a8332cf8cd.rlib' '--extern' 'serde=/home/runner/work/supergreen/supergreen/target/debug/deps/libserde-e8d25659f6b92157.rlib' '--extern' 'serde_jsonlines=/home/runner/work/supergreen/supergreen/target/debug/deps/libserde_jsonlines-b9abadeabde9e733.rlib' '--extern' 'serde_json=/home/runner/work/supergreen/supergreen/target/debug/deps/libserde_json-363ab0e04bb26d42.rlib' '--extern' 'sha256=/home/runner/work/supergreen/supergreen/target/debug/deps/libsha256-fa64a708960a507e.rlib' '--extern' 'szyk=/home/runner/work/supergreen/supergreen/target/debug/deps/libszyk-15383af1e4a4ba93.rlib' '--extern' 'tokio=/home/runner/work/supergreen/supergreen/target/debug/deps/libtokio-5bf6c2b013a5c1c7.rlib' '--extern' 'toml=/home/runner/work/supergreen/supergreen/target/debug/deps/libtoml-5724af712b40fb32.rlib' '-L' 'native=/home/runner/work/supergreen/supergreen/target/debug/build/ring-eff462b40b2a19a0/out' cargo-green/src/main.rs       1> >(sed 's/^/::STDOUT:: /')       2> >(sed 's/^/::STDERR:: /' >&2)\"" did not complete successfully: exit code: 1
        //test|cargo-green|0.8.0|f273b3fc9f002200@34172] ✖ ------
        //test|cargo-green|0.8.0|f273b3fc9f002200@34172] ✖  > [cwd-test-cargo-green-0.8.0-f273b3fc9f002200-cargo-green-src-main-rs 2/2] RUN   --mount=from=out-efd9ef597eff750c,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libaho_corasick-efd9ef597eff750c.rlib,source=/libaho_corasick-efd9ef597eff750c.rlib   --mount=from=out-1013858726d44416,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libanstream-1013858726d44416.rlib,source=/libanstream-1013858726d44416.rlib   --mount=from=out-2ffce3dd7e3a0f17,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libanstyle-2ffce3dd7e3a0f17.rlib,source=/libanstyle-2ffce3dd7e3a0f17.rlib   --mount=from=out-534aae5427a00bf0,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libanstyle_parse-534aae5427a00bf0.rlib,source=/libanstyle_parse-534aae5427a00bf0.rlib   --mount=from=out-2670247d99f37bb1,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libanstyle_query-2670247d99f37bb1.rlib,source=/libanstyle_query-2670247d99f37bb1.rlib   --mount=from=out-8994ddf264b506dd,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libanyhow-8994ddf264b506dd.rlib,source=/libanyhow-8994ddf264b506dd.rlib   --mount=from=out-868dc96d8980862a,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libassertx-868dc96d8980862a.rlib,source=/libassertx-868dc96d8980862a.rlib   --mount=from=out-ebc745c1619b1300,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libasync_trait-ebc745c1619b1300.so,source=/libasync_trait-ebc745c1619b1300.so   --mount=from=out-078ecd23dfe6b8b9,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libbase64-078ecd23dfe6b8b9.rlib,source=/libbase64-078ecd23dfe6b8b9.rlib   --mount=from=out-7769a6e70e6e0ed0,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libblock_buffer-7769a6e70e6e0ed0.rlib,source=/libblock_buffer-7769a6e70e6e0ed0.rlib   --mount=from=out-c2a53c112edb335f,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libbytes-c2a53c112edb335f.rlib,source=/libbytes-c2a53c112edb335f.rlib   --mount=from=out-7471cc044e43fae8,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libcamino-7471cc044e43fae8.rlib,source=/libcamino-7471cc044e43fae8.rlib   --mount=from=out-5d8cf5fd7778c4e1,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libcfg_if-5d8cf5fd7778c4e1.rlib,source=/libcfg_if-5d8cf5fd7778c4e1.rlib   --mount=from=out-a6abfa0520cb84fd,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libcolorchoice-a6abfa0520cb84fd.rlib,source=/libcolorchoice-a6abfa0520cb84fd.rlib   --mount=from=out-e0814ceea2bbb2aa,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libconvert_case-e0814ceea2bbb2aa.rlib,source=/libconvert_case-e0814ceea2bbb2aa.rlib   --mount=from=out-8cd0aee74198d6f1,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libcpufeatures-8cd0aee74198d6f1.rlib,source=/libcpufeatures-8cd0aee74198d6f1.rlib   --mount=from=out-f9c031fd8603f150,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libcrypto_common-f9c031fd8603f150.rlib,source=/libcrypto_common-f9c031fd8603f150.rlib   --mount=from=out-4d70bb079fd47203,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libdiff-4d70bb079fd47203.rlib,source=/libdiff-4d70bb079fd47203.rlib   --mount=from=out-234c00e05642df73,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libdigest-234c00e05642df73.rlib,source=/libdigest-234c00e05642df73.rlib   --mount=from=out-8591eec41c64b07d,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libenv_filter-8591eec41c64b07d.rlib,source=/libenv_filter-8591eec41c64b07d.rlib   --mount=from=out-8bf0bc253e2b5e56,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libenv_logger-8bf0bc253e2b5e56.rlib,source=/libenv_logger-8bf0bc253e2b5e56.rlib   --mount=from=out-7819ae4dc35a22a8,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libequivalent-7819ae4dc35a22a8.rlib,source=/libequivalent-7819ae4dc35a22a8.rlib   --mount=from=out-1611f0d246b3c5ac,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libfnv-1611f0d246b3c5ac.rlib,source=/libfnv-1611f0d246b3c5ac.rlib   --mount=from=out-fd2d2c0efb0c7fb7,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libform_urlencoded-fd2d2c0efb0c7fb7.rlib,source=/libform_urlencoded-fd2d2c0efb0c7fb7.rlib   --mount=from=out-7d5afd56343d7d1b,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libfutures-7d5afd56343d7d1b.rlib,source=/libfutures-7d5afd56343d7d1b.rlib   --mount=from=out-b2d17e46bc25df68,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libfutures_channel-b2d17e46bc25df68.rlib,source=/libfutures_channel-b2d17e46bc25df68.rlib   --mount=from=out-9641ec2c244a0577,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libfutures_core-9641ec2c244a0577.rlib,source=/libfutures_core-9641ec2c244a0577.rlib   --mount=from=out-1b140496b745f14e,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libfutures_executor-1b140496b745f14e.rlib,source=/libfutures_executor-1b140496b745f14e.rlib   --mount=from=out-ec6fa6a798f2079e,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libfutures_io-ec6fa6a798f2079e.rlib,source=/libfutures_io-ec6fa6a798f2079e.rlib   --mount=from=out-7d57e90f0fe4c42e,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libfutures_macro-7d57e90f0fe4c42e.so,source=/libfutures_macro-7d57e90f0fe4c42e.so   --mount=from=out-640fb4667002ee05,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libfutures_sink-640fb4667002ee05.rlib,source=/libfutures_sink-640fb4667002ee05.rlib   --mount=from=out-b221d63afad2eaa1,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libfutures_task-b221d63afad2eaa1.rlib,source=/libfutures_task-b221d63afad2eaa1.rlib   --mount=from=out-ee9fc1b216db5748,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libfutures_util-ee9fc1b216db5748.rlib,source=/libfutures_util-ee9fc1b216db5748.rlib   --mount=from=out-f9c5a3b1307852b9,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libgeneric_array-f9c5a3b1307852b9.rlib,source=/libgeneric_array-f9c5a3b1307852b9.rlib   --mount=from=out-9ec8c6cf62c7dd32,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libgetrandom-9ec8c6cf62c7dd32.rlib,source=/libgetrandom-9ec8c6cf62c7dd32.rlib   --mount=from=out-4892313c0b2fe73f,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libhashbrown-4892313c0b2fe73f.rlib,source=/libhashbrown-4892313c0b2fe73f.rlib   --mount=from=out-d01e622a824924a1,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libhex-d01e622a824924a1.rlib,source=/libhex-d01e622a824924a1.rlib   --mount=from=out-553e02f09d173e01,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libhome-553e02f09d173e01.rlib,source=/libhome-553e02f09d173e01.rlib   --mount=from=out-f03849329dfa70fb,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libhttp-f03849329dfa70fb.rlib,source=/libhttp-f03849329dfa70fb.rlib   --mount=from=out-8479c453f97f4e85,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libhttp_body-8479c453f97f4e85.rlib,source=/libhttp_body-8479c453f97f4e85.rlib   --mount=from=out-60a1c3b03e67f2da,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libhttp_body_util-60a1c3b03e67f2da.rlib,source=/libhttp_body_util-60a1c3b03e67f2da.rlib   --mount=from=out-f994542ed298da3a,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libhttparse-f994542ed298da3a.rlib,source=/libhttparse-f994542ed298da3a.rlib   --mount=from=out-3a1f92ab6a57317d,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libhumantime-3a1f92ab6a57317d.rlib,source=/libhumantime-3a1f92ab6a57317d.rlib   --mount=from=out-9ca0d06f97fa947f,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libhyper-9ca0d06f97fa947f.rlib,source=/libhyper-9ca0d06f97fa947f.rlib   --mount=from=out-59e11dfc724da753,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libhyper_rustls-59e11dfc724da753.rlib,source=/libhyper_rustls-59e11dfc724da753.rlib   --mount=from=out-38eea8d7c8cb14d5,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libhyper_util-38eea8d7c8cb14d5.rlib,source=/libhyper_util-38eea8d7c8cb14d5.rlib   --mount=from=out-95e889519cbec69f,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libidna-95e889519cbec69f.rlib,source=/libidna-95e889519cbec69f.rlib   --mount=from=out-91c295e24598cadb,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libindexmap-91c295e24598cadb.rlib,source=/libindexmap-91c295e24598cadb.rlib   --mount=from=out-8328439dca13d52d,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libipnet-8328439dca13d52d.rlib,source=/libipnet-8328439dca13d52d.rlib   --mount=from=out-254bc9c5bee28d57,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libis_terminal_polyfill-254bc9c5bee28d57.rlib,source=/libis_terminal_polyfill-254bc9c5bee28d57.rlib   --mount=from=out-9509d7e57962fa54,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libitoa-9509d7e57962fa54.rlib,source=/libitoa-9509d7e57962fa54.rlib   --mount=from=out-f22dbd37e20c377d,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libkinded-f22dbd37e20c377d.rlib,source=/libkinded-f22dbd37e20c377d.rlib   --mount=from=out-21df50878877f49e,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libkinded_macros-21df50878877f49e.so,source=/libkinded_macros-21df50878877f49e.so   --mount=from=out-5fa5c88e5c24b7f9,target=/home/runner/work/supergreen/supergreen/target/debug/deps/liblibc-5fa5c88e5c24b7f9.rlib,source=/liblibc-5fa5c88e5c24b7f9.rlib   --mount=from=out-6865073cb12eb520,target=/home/runner/work/supergreen/supergreen/target/debug/deps/liblog-6865073cb12eb520.rlib,source=/liblog-6865073cb12eb520.rlib   --mount=from=out-8535514d537a2d44,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libmemchr-8535514d537a2d44.rlib,source=/libmemchr-8535514d537a2d44.rlib   --mount=from=out-070b3d676890c6be,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libmime-070b3d676890c6be.rlib,source=/libmime-070b3d676890c6be.rlib   --mount=from=out-86b1c95cf812d632,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libmio-86b1c95cf812d632.rlib,source=/libmio-86b1c95cf812d632.rlib   --mount=from=out-2d0a379f7162102a,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libnutype-2d0a379f7162102a.rlib,source=/libnutype-2d0a379f7162102a.rlib   --mount=from=out-d6de9cc1d608a71e,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libnutype_macros-d6de9cc1d608a71e.so,source=/libnutype_macros-d6de9cc1d608a71e.so   --mount=from=out-a181560b04e888f8,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libonce_cell-a181560b04e888f8.rlib,source=/libonce_cell-a181560b04e888f8.rlib   --mount=from=out-f34b63eb41f0a092,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libpercent_encoding-f34b63eb41f0a092.rlib,source=/libpercent_encoding-f34b63eb41f0a092.rlib   --mount=from=out-06c29f380e1bc80d,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libpin_project-06c29f380e1bc80d.rlib,source=/libpin_project-06c29f380e1bc80d.rlib   --mount=from=out-4ef922d04e507866,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libpin_project_internal-4ef922d04e507866.so,source=/libpin_project_internal-4ef922d04e507866.so   --mount=from=out-fb729104c225308b,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libpin_project_lite-fb729104c225308b.rlib,source=/libpin_project_lite-fb729104c225308b.rlib   --mount=from=out-2affb3f3e144b9a6,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libpin_utils-2affb3f3e144b9a6.rlib,source=/libpin_utils-2affb3f3e144b9a6.rlib   --mount=from=out-8016036a38e95dc8,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libpretty_assertions-8016036a38e95dc8.rlib,source=/libpretty_assertions-8016036a38e95dc8.rlib   --mount=from=out-4c24e61a51cb79c1,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libproc_macro2-4c24e61a51cb79c1.rlib,source=/libproc_macro2-4c24e61a51cb79c1.rlib   --mount=from=out-5a676f4bf2c81bf1,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libquote-5a676f4bf2c81bf1.rlib,source=/libquote-5a676f4bf2c81bf1.rlib   --mount=from=out-24912ce4247f528a,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libregex-24912ce4247f528a.rlib,source=/libregex-24912ce4247f528a.rlib   --mount=from=out-2dd198ea027a8221,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libregex_automata-2dd198ea027a8221.rlib,source=/libregex_automata-2dd198ea027a8221.rlib   --mount=from=out-cf706bc1f5acd820,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libregex_syntax-cf706bc1f5acd820.rlib,source=/libregex_syntax-cf706bc1f5acd820.rlib   --mount=from=out-d912a0b62e27096a,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libreqwest-d912a0b62e27096a.rlib,source=/libreqwest-d912a0b62e27096a.rlib   --mount=from=out-b73e870bb5285791,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libring-b73e870bb5285791.rlib,source=/libring-b73e870bb5285791.rlib   --mount=from=out-02ab48bfebb94691,target=/home/runner/work/supergreen/supergreen/target/debug/deps/librustc_version-02ab48bfebb94691.rlib,source=/librustc_version-02ab48bfebb94691.rlib   --mount=from=out-a52071a8332cf8cd,target=/home/runner/work/supergreen/supergreen/target/debug/deps/librustflags-a52071a8332cf8cd.rlib,source=/librustflags-a52071a8332cf8cd.rlib   --mount=from=out-77240bfa0039586a,target=/home/runner/work/supergreen/supergreen/target/debug/deps/librustls-77240bfa0039586a.rlib,source=/librustls-77240bfa0039586a.rlib   --mount=from=out-e86f2e14e12c3b79,target=/home/runner/work/supergreen/supergreen/target/debug/deps/librustls_pemfile-e86f2e14e12c3b79.rlib,source=/librustls_pemfile-e86f2e14e12c3b79.rlib   --mount=from=out-1fc8c150aa3ef696,target=/home/runner/work/supergreen/supergreen/target/debug/deps/librustls_pki_types-1fc8c150aa3ef696.rlib,source=/librustls_pki_types-1fc8c150aa3ef696.rlib   --mount=from=out-81015048e67b40bb,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libryu-81015048e67b40bb.rlib,source=/libryu-81015048e67b40bb.rlib   --mount=from=out-c71e56068c261309,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libsemver-c71e56068c261309.rlib,source=/libsemver-c71e56068c261309.rlib   --mount=from=out-e8d25659f6b92157,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libserde-e8d25659f6b92157.rlib,source=/libserde-e8d25659f6b92157.rlib   --mount=from=out-fb2ab027acdb0984,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libserde_derive-fb2ab027acdb0984.so,source=/libserde_derive-fb2ab027acdb0984.so   --mount=from=out-363ab0e04bb26d42,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libserde_json-363ab0e04bb26d42.rlib,source=/libserde_json-363ab0e04bb26d42.rlib   --mount=from=out-b9abadeabde9e733,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libserde_jsonlines-b9abadeabde9e733.rlib,source=/libserde_jsonlines-b9abadeabde9e733.rlib   --mount=from=out-fa8b3a8a01890f18,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libserde_spanned-fa8b3a8a01890f18.rlib,source=/libserde_spanned-fa8b3a8a01890f18.rlib   --mount=from=out-f077663a212e2ed0,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libserde_urlencoded-f077663a212e2ed0.rlib,source=/libserde_urlencoded-f077663a212e2ed0.rlib   --mount=from=out-655d479d66242fa3,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libsha2-655d479d66242fa3.rlib,source=/libsha2-655d479d66242fa3.rlib   --mount=from=out-fa64a708960a507e,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libsha256-fa64a708960a507e.rlib,source=/libsha256-fa64a708960a507e.rlib   --mount=from=out-67451ca025a7afe5,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libsignal_hook_registry-67451ca025a7afe5.rlib,source=/libsignal_hook_registry-67451ca025a7afe5.rlib   --mount=from=out-c0851e486350ed48,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libslab-c0851e486350ed48.rlib,source=/libslab-c0851e486350ed48.rlib   --mount=from=out-a5beaec36b002750,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libsmallvec-a5beaec36b002750.rlib,source=/libsmallvec-a5beaec36b002750.rlib   --mount=from=out-cdc8b3a1996dc0eb,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libsocket2-cdc8b3a1996dc0eb.rlib,source=/libsocket2-cdc8b3a1996dc0eb.rlib   --mount=from=out-844824aad34f0139,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libspin-844824aad34f0139.rlib,source=/libspin-844824aad34f0139.rlib   --mount=from=out-8a41f0946d132362,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libsubtle-8a41f0946d132362.rlib,source=/libsubtle-8a41f0946d132362.rlib   --mount=from=out-e9505586f90a1f77,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libsyn-e9505586f90a1f77.rlib,source=/libsyn-e9505586f90a1f77.rlib   --mount=from=out-4b1cad807b331367,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libsync_wrapper-4b1cad807b331367.rlib,source=/libsync_wrapper-4b1cad807b331367.rlib   --mount=from=out-15383af1e4a4ba93,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libszyk-15383af1e4a4ba93.rlib,source=/libszyk-15383af1e4a4ba93.rlib   --mount=from=out-577e4c0236cc740b,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libtinyvec-577e4c0236cc740b.rlib,source=/libtinyvec-577e4c0236cc740b.rlib   --mount=from=out-c12cee6632de4a31,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libtinyvec_macros-c12cee6632de4a31.rlib,source=/libtinyvec_macros-c12cee6632de4a31.rlib   --mount=from=out-5bf6c2b013a5c1c7,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libtokio-5bf6c2b013a5c1c7.rlib,source=/libtokio-5bf6c2b013a5c1c7.rlib   --mount=from=out-e4751b2a1ec5c0b3,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libtokio_macros-e4751b2a1ec5c0b3.so,source=/libtokio_macros-e4751b2a1ec5c0b3.so   --mount=from=out-dc38bac6773c9fe8,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libtokio_rustls-dc38bac6773c9fe8.rlib,source=/libtokio_rustls-dc38bac6773c9fe8.rlib   --mount=from=out-5724af712b40fb32,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libtoml-5724af712b40fb32.rlib,source=/libtoml-5724af712b40fb32.rlib   --mount=from=out-480b7a196f87e82f,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libtoml_datetime-480b7a196f87e82f.rlib,source=/libtoml_datetime-480b7a196f87e82f.rlib   --mount=from=out-2099a92a0b7c150a,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libtoml_edit-2099a92a0b7c150a.rlib,source=/libtoml_edit-2099a92a0b7c150a.rlib   --mount=from=out-dbe21d19380800f7,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libtower-dbe21d19380800f7.rlib,source=/libtower-dbe21d19380800f7.rlib   --mount=from=out-55167ad84f5cb165,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libtower_layer-55167ad84f5cb165.rlib,source=/libtower_layer-55167ad84f5cb165.rlib   --mount=from=out-430fbffc2906c9d3,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libtower_service-430fbffc2906c9d3.rlib,source=/libtower_service-430fbffc2906c9d3.rlib   --mount=from=out-0fab37dae9fe96fc,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libtracing-0fab37dae9fe96fc.rlib,source=/libtracing-0fab37dae9fe96fc.rlib   --mount=from=out-ea979041f533d552,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libtracing_core-ea979041f533d552.rlib,source=/libtracing_core-ea979041f533d552.rlib   --mount=from=out-51ff99ede286dcca,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libtry_lock-51ff99ede286dcca.rlib,source=/libtry_lock-51ff99ede286dcca.rlib   --mount=from=out-1cf05e3a721cf676,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libtypenum-1cf05e3a721cf676.rlib,source=/libtypenum-1cf05e3a721cf676.rlib   --mount=from=out-98f019b73d41c11b,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libunicode_bidi-98f019b73d41c11b.rlib,source=/libunicode_bidi-98f019b73d41c11b.rlib   --mount=from=out-542f5604bbfe0ac2,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libunicode_ident-542f5604bbfe0ac2.rlib,source=/libunicode_ident-542f5604bbfe0ac2.rlib   --mount=from=out-22c06b76a9c82ac0,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libunicode_normalization-22c06b76a9c82ac0.rlib,source=/libunicode_normalization-22c06b76a9c82ac0.rlib   --mount=from=out-9dbbc876f99d9410,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libunicode_segmentation-9dbbc876f99d9410.rlib,source=/libunicode_segmentation-9dbbc876f99d9410.rlib   --mount=from=out-01dc8a00465407f6,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libuntrusted-01dc8a00465407f6.rlib,source=/libuntrusted-01dc8a00465407f6.rlib   --mount=from=out-a479f9dd84e5db80,target=/home/runner/work/supergreen/supergreen/target/debug/deps/liburl-a479f9dd84e5db80.rlib,source=/liburl-a479f9dd84e5db80.rlib   --mount=from=out-6085bc4262171f66,target=/home/runner/work/supergreen/supergreen/target/debug/deps/liburlencoding-6085bc4262171f66.rlib,source=/liburlencoding-6085bc4262171f66.rlib   --mount=from=out-0fd400427d500834,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libutf8parse-0fd400427d500834.rlib,source=/libutf8parse-0fd400427d500834.rlib   --mount=from=out-329c354c918c8800,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libwant-329c354c918c8800.rlib,source=/libwant-329c354c918c8800.rlib   --mount=from=out-caf9440bdb3aa7ed,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libwebpki-caf9440bdb3aa7ed.rlib,source=/libwebpki-caf9440bdb3aa7ed.rlib   --mount=from=out-05c6048285b4fe4e,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libwebpki_roots-05c6048285b4fe4e.rlib,source=/libwebpki_roots-05c6048285b4fe4e.rlib   --mount=from=out-122df945fd947c3b,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libwinnow-122df945fd947c3b.rlib,source=/libwinnow-122df945fd947c3b.rlib   --mount=from=out-d6dc2b3b3f64b3a5,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libyansi-d6dc2b3b3f64b3a5.rlib,source=/libyansi-d6dc2b3b3f64b3a5.rlib   --mount=from=out-a3c891722edb5521,target=/home/runner/work/supergreen/supergreen/target/debug/deps/libzeroize-a3c891722edb5521.rlib,source=/libzeroize-a3c891722edb5521.rlib   --mount=from=cwd-f273b3fc9f002200,target=/tmp/cwd-f273b3fc9f002200     set -eux  && mkdir -p /home/runner/work/supergreen/supergreen  && cp -pr /tmp/cwd-f273b3fc9f002200/* /home/runner/work/supergreen/supergreen/  && cd /home/runner/work/supergreen/supergreen  && export CARGO="$(which cargo)"  && /bin/bash -c "rustc '--crate-name' 'cargo_green' '--edition' '2021' '--error-format' 'json' '--json' 'diagnostic-rendered-ansi,artifacts,future-incompat' '--emit' 'dep-info,link' '-C' 'embed-bitcode=no' '-C' 'debuginfo=2' '--test' '--check-cfg' 'cfg(docsrs)' '--check-cfg' 'cfg(feature, values())' '-C' 'metadata=f273b3fc9f002200' '-C' 'extra-filename=-f273b3fc9f002200' '--out-dir' '/home/runner/work/supergreen/supergreen/target/debug/deps' '-C' 'incremental=/home/runner/work/supergreen/supergreen/target/debug/incremental' '-L' 'dependency=/home/runner/work/supergreen/supergreen/target/debug/deps' '--extern' 'anyhow=/home/runner/work/supergreen/supergreen/target/debug/deps/libanyhow-8994ddf264b506dd.rlib' '--extern' 'assertx=/home/runner/work/supergreen/supergreen/target/debug/deps/libassertx-868dc96d8980862a.rlib' '--extern' 'camino=/home/runner/work/supergreen/supergreen/target/debug/deps/libcamino-7471cc044e43fae8.rlib' '--extern' 'env_logger=/home/runner/work/supergreen/supergreen/target/debug/deps/libenv_logger-8bf0bc253e2b5e56.rlib' '--extern' 'futures=/home/runner/work/supergreen/supergreen/target/debug/deps/libfutures-7d5afd56343d7d1b.rlib' '--extern' 'home=/home/runner/work/supergreen/supergreen/target/debug/deps/libhome-553e02f09d173e01.rlib' '--extern' 'log=/home/runner/work/supergreen/supergreen/target/debug/deps/liblog-6865073cb12eb520.rlib' '--extern' 'nutype=/home/runner/work/supergreen/supergreen/target/debug/deps/libnutype-2d0a379f7162102a.rlib' '--extern' 'pretty_assertions=/home/runner/work/supergreen/supergreen/target/debug/deps/libpretty_assertions-8016036a38e95dc8.rlib' '--extern' 'reqwest=/home/runner/work/supergreen/supergreen/target/debug/deps/libreqwest-d912a0b62e27096a.rlib' '--extern' 'rustc_version=/home/runner/work/supergreen/supergreen/target/debug/deps/librustc_version-02ab48bfebb94691.rlib' '--extern' 'rustflags=/home/runner/work/supergreen/supergreen/target/debug/deps/librustflags-a52071a8332cf8cd.rlib' '--extern' 'serde=/home/runner/work/supergreen/supergreen/target/debug/deps/libserde-e8d25659f6b92157.rlib' '--extern' 'serde_jsonlines=/home/runner/work/supergreen/supergreen/target/debug/deps/libserde_jsonlines-b9abadeabde9e733.rlib' '--extern' 'serde_json=/home/runner/work/supergreen/supergreen/target/debug/deps/libserde_json-363ab0e04bb26d42.rlib' '--extern' 'sha256=/home/runner/work/supergreen/supergreen/target/debug/deps/libsha256-fa64a708960a507e.rlib' '--extern' 'szyk=/home/runner/work/supergreen/supergreen/target/debug/deps/libszyk-15383af1e4a4ba93.rlib' '--extern' 'tokio=/home/runner/work/supergreen/supergreen/target/debug/deps/libtokio-5bf6c2b013a5c1c7.rlib' '--extern' 'toml=/home/runner/work/supergreen/supergreen/target/debug/deps/libtoml-5724af712b40fb32.rlib' '-L' 'native=/home/runner/work/supergreen/supergreen/target/debug/build/ring-eff462b40b2a19a0/out' cargo-green/src/main.rs       1> >(sed 's/^/::STDOUT:: /')       2> >(sed 's/^/::STDERR:: /' >&2)":
        //test|cargo-green|0.8.0|f273b3fc9f002200@34172] ✖ 0.459 cp: cannot create regular file '/home/runner/work/supergreen/supergreen/target/debug/deps/libunicode_segmentation-9dbbc876f99d9410.rlib': Read-only file system
        //test|cargo-green|0.8.0|f273b3fc9f002200@34172] ✖ 0.459 cp: cannot create regular file '/home/runner/work/supergreen/supergreen/target/debug/deps/liburl-a479f9dd84e5db80.rlib': Read-only file system
    }

    for (var, val) in env::vars() {
        let (pass, skip, only_buildrs) = pass_env(var.as_str());
        if pass || (crate_name == BUILDRS_CRATE_NAME && only_buildrs) {
            if skip {
                log::debug!(target: &krate, "not forwarding env: {var}={val}");
                continue;
            }
            let val = safeify(val);
            if var == "CARGO_ENCODED_RUSTFLAGS" {
                let dec: Vec<_> = rustflags::from_env().collect();
                log::debug!(target: &krate, "env is set: {var}={val} ({dec:?})");
            } else {
                log::debug!(target: &krate, "env is set: {var}={val}");
            }
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
            log::warn!(target: &krate, "passing ${var}={v:?} env through");
            rustc_block.push_str(&format!("        {var}={v:?} \\\n"));
        }
    }

    // TODO: keep only paths that we explicitly mount or copy
    if false {
        // https://github.com/maelstrom-software/maelstrom/blob/ef90f8a990722352e55ef1a2f219ef0fc77e7c8c/crates/maelstrom-util/src/elf.rs#L4
        for var in ["PATH", "DYLD_FALLBACK_LIBRARY_PATH", "LD_LIBRARY_PATH", "LIBPATH"] {
            let Ok(val) = env::var(var) else { continue };
            log::debug!(target: &krate, "system env set (skipped): ${var}={val:?}");
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
    out_block.push_str(&format!("COPY --from={rustc_stage} {out_dir}/*-{metadata}* /\n"));
    // NOTE: -C extra-filename=-${metadata} (starts with dash)
    // TODO: use extra filename here for fwd compat
    md.push_block(&out_stage, out_block);

    let md = md; // Drop mut

    let mut blocks = String::new();
    let mut visited_cratesio_stages = BTreeSet::new();
    for extern_md_path in extern_md_paths {
        log::info!(target: &krate, "opening (RO) extern's md {extern_md_path}");
        let md_raw = fs::read_to_string(&extern_md_path)
            .map_err(|e| anyhow!("Failed reading Md {extern_md_path}: {e}"))?;
        let extern_md = Md::from_str(&md_raw)?;
        extern_md.append_blocks(&mut blocks, &mut visited_cratesio_stages)?;
        blocks.push('\n');
    }
    md.append_blocks(&mut blocks, &mut visited_cratesio_stages)?;

    {
        let md_path = Utf8Path::new(&target_path).join(format!("{crate_name}-{metadata}.toml"));
        let md_ser = md.to_string_pretty()?;

        log::info!(target: &krate, "opening (RW) crate's md {md_path}");
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
        let dockerfile =
            Utf8Path::new(&target_path).join(format!("{crate_name}-{metadata}.Dockerfile"));

        let syntax = syntax().await.trim_start_matches("docker-image://");
        let mut header = format!("# syntax={syntax}\n");
        header.push_str(&format!("# Generated by {REPO} version {VSN}\n"));
        let root = md.rust_stage().expect("Stage 'rust' is the root stage");
        header.push_str(&format!("{}\n", root.script));
        header.push_str(&blocks);

        log::info!(target: &krate, "opening (RW) crate dockerfile {dockerfile}");
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
        // [2024-08-22T22:27:01Z INFO  bin|cargo-green|0.8.0|efe5575298075b07] blblblbl contains DirEntry("/home/runner/work/supergreen/supergreen/.git")
        // [2024-08-22T22:27:01Z INFO  bin|cargo-green|0.8.0|efe5575298075b07] blblblbl contains DirEntry("/home/runner/work/supergreen/supergreen/hack")
        // [2024-08-22T22:27:01Z INFO  bin|cargo-green|0.8.0|efe5575298075b07] blblblbl contains DirEntry("/home/runner/work/supergreen/supergreen/Cargo.lock")
        // [2024-08-22T22:27:01Z INFO  bin|cargo-green|0.8.0|efe5575298075b07] blblblbl contains DirEntry("/home/runner/work/supergreen/supergreen/target")
        // [2024-08-22T22:27:01Z INFO  bin|cargo-green|0.8.0|efe5575298075b07] blblblbl contains DirEntry("/home/runner/work/supergreen/supergreen/LICENSE")
        // [2024-08-22T22:27:01Z INFO  bin|cargo-green|0.8.0|efe5575298075b07] blblblbl contains DirEntry("/home/runner/work/supergreen/supergreen/README.md")
        // [2024-08-22T22:27:01Z INFO  bin|cargo-green|0.8.0|efe5575298075b07] blblblbl contains DirEntry("/home/runner/work/supergreen/supergreen/cargo-green")
        // [2024-08-22T22:27:01Z INFO  bin|cargo-green|0.8.0|efe5575298075b07] blblblbl contains DirEntry("/home/runner/work/supergreen/supergreen/Cargo.toml")
        // [2024-08-22T22:27:01Z INFO  bin|cargo-green|0.8.0|efe5575298075b07] blblblbl contains DirEntry("/home/runner/work/supergreen/supergreen/rustfmt.toml")
        // [2024-08-22T22:27:01Z INFO  bin|cargo-green|0.8.0|efe5575298075b07] blblblbl contains DirEntry("/home/runner/work/supergreen/supergreen/.gitignore")
        // [2024-08-22T22:27:01Z INFO  bin|cargo-green|0.8.0|efe5575298075b07] blblblbl contains DirEntry("/home/runner/work/supergreen/supergreen/.github")
        fs::write(&ignore, "/target\n")
            .map_err(|e| anyhow!("Failed creating dockerignore {ignore}: {e}"))?;

        if debug.is_some() {
            log::info!(target: &krate, "dockerfile: {dockerfile}");
            match fs::read_to_string(&dockerfile) {
                Ok(data) => data,
                Err(e) => e.to_string(),
            }
            .lines()
            .filter(|x| !x.is_empty())
            .for_each(|line| log::debug!(target: &krate, "❯ {line}"));
        }

        dockerfile
    };

    // TODO: use tracing instead:
    // https://docs.rs/tracing-subscriber/latest/tracing_subscriber/fmt/struct.Subscriber.html
    // https://crates.io/crates/tracing-appender
    // https://github.com/tugglecore/rust-tracing-primer
    // TODO: `cargo green -v{N+1} ..` starts a TUI showing colored logs on above `cargo -v{N} ..`

    let command = runner();
    if command == "none" {
        return fallback.await;
    }
    let code = build(krate, command, &dockerfile, out_stage, &md.contexts, out_dir).await?;
    if let Some(incremental) = incremental {
        let _ = build(krate, command, &dockerfile, incremental_stage, &md.contexts, incremental)
            .await
            .inspect_err(|e| log::warn!(target: &krate, "Error fetching incremental data: {e}"));
    }

    if code != Some(0) && debug.is_none() {
        log::warn!(target: &krate, "Falling back...");
        let res = fallback.await; // Bubble up actual error & outputs
        if res.is_ok() {
            log::error!(target: &krate, "BUG found!");
            eprintln!("Found a bug in this script! Falling back... (logs: {debug:?})");
        }
        return res;
    }

    Ok(exit_code(code))
}

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

#[inline]
fn file_exists(path: &Utf8Path) -> Result<bool> {
    match path.metadata().map(|md| md.is_file()) {
        Ok(b) => Ok(b),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(anyhow!("Failed to `stat {path}`: {e}")),
    }
}

#[inline]
fn file_exists_and_is_not_empty(path: &Utf8Path) -> Result<bool> {
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

#[inline]
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

fn crate_out_name(name: &str) -> String {
    Utf8Path::new(name)
        .parent()
        .and_then(|x| x.file_name())
        .and_then(|x| x.rsplit_once('-'))
        .map(|(_, x)| x)
        .map(|x| format!("crate_out-{x}"))
        .expect("PROOF: suffix is /out")
}

fn copy_dir_all<P: AsRef<Path>>(src: P, dst: P, ignore_prefix: Option<&Utf8PathBuf>) -> Result<()> {
    let dst = dst.as_ref();
    let src = src.as_ref();

    if dst.exists() {
        log::warn!("blblblbl dst {dst:?} exists, skipping copy");
        return Ok(());
    }

    fs::create_dir_all(dst).map_err(|e| anyhow!("Failed `mkdir -p {dst:?}`: {e}"))?;

    log::warn!("blblblbl :eyes: {src:?} [{dst:?}] ({ignore_prefix:?})");

    // TODO: deterministic iteration
    for entry in fs::read_dir(src).map_err(|e| anyhow!("Failed reading dir {src:?}: {e}"))? {
        let entry = entry?;
        let fpath = entry.path();
        let fname = entry.file_name();
        let ty = entry.file_type().map_err(|e| anyhow!("Failed typing {entry:?}: {e}"))?;
        log::warn!("blblblbl ?cp {fpath:?} matches? {ignore_prefix:?}");
        if ignore_prefix.map(|p| fpath.starts_with(p)).unwrap_or(false) {
            log::warn!("blblblbl !cp {fpath:?} {:?}", dst.join(&fname));
            continue; // Skip all paths under ignore_prefix
        }
        if ty.is_dir() {
            if fname == ".git" {
                log::warn!("blblblbl !.git {fpath:?} {:?}", dst.join(&fname));
                continue; // Skip copying .git dir
            }
            copy_dir_all(fpath, dst.join(fname), ignore_prefix)?;
        } else {
            log::warn!("blblblbl cp {fpath:?} {:?}", dst.join(&fname));
            fs::copy(&fpath, dst.join(fname)).map_err(|e| {
                anyhow!("Failed `cp {fpath:?} {dst:?}` ({:?}): {e}", entry.metadata())
            })?;
        }
    }
    Ok(())
}
