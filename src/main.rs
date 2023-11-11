use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fmt::Write as FmtWrite,
    fs::{self, create_dir_all, read_dir, read_to_string, File, OpenOptions},
    io::{BufRead, BufReader, ErrorKind},
    process::{Command, ExitCode, Stdio},
    time::Instant,
    unreachable,
};

use anyhow::{bail, Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use env_logger::{Env, Target};
use envs::{log_path, RUSTCBUILDX_LOG, RUSTCBUILDX_LOG_STYLE};
use log::{debug, error, info, warn};
use mktemp::Temp;
use regex::Regex;

use crate::{
    envs::{docker_image, docker_syntax, is_debug, RUSTCBUILDX_LOG_IF_CRATE_NAME},
    parse::RustcArgs,
    pops::Popped,
};

mod envs;
mod parse;
mod pops;

// NOTE: this RUSTC_WRAPPER program only ever gets called by `cargo`, so we save
//       ourselves some trouble and assume std::path::{Path, PathBuf} are UTF-8.
//       Or in the words of this crate: https://github.com/camino-rs/camino/tree/8bec62382e1bce1326ee48f6bf93c46e7a4fde0b#:~:text=there%20are%20already%20many%20systems%2C%20such%20as%20cargo%2C%20that%20only%20support%20utf-8%20paths.%20if%20your%20own%20tool%20interacts%20with%20any%20such%20system%2C%20you%20can%20assume%20that%20paths%20are%20valid%20utf-8%20without%20creating%20any%20additional%20burdens%20on%20consumers.

fn main() -> ExitCode {
    faillible_main().unwrap_or(ExitCode::FAILURE)
}

fn faillible_main() -> Result<ExitCode> {
    let first_few_args = env::args().skip(1).take(3).collect::<Vec<String>>();
    let first_few_args = first_few_args.iter().map(String::as_str).collect::<Vec<_>>();
    match &first_few_args[..] {
        [] | ["-h" | "--help" |"--version"] => help(),
        [rustc, "-", ..] | [rustc, _ /*driver*/, "-", ..] => {
            return call_rustc(rustc, || env::args().skip(2));
        }
        [rustc, "--crate-name", crate_name, ..] => {
            return bake_rustc(crate_name, env::args().skip(2).collect(), || {
                call_rustc(rustc, || env::args().skip(2))
            })
            .map_err(|e| {
                error!(target:crate_name, "Failure: {e}");
                eprintln!("Failure: {e}");
                e
            });
        }
        _ => {}
    }

    Ok(ExitCode::SUCCESS)
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

fn help() {
    println!(
        "{name} {version}: {description}\n{repository}",
        name = env!("CARGO_PKG_NAME"),
        description = env!("CARGO_PKG_DESCRIPTION"),
        version = env!("CARGO_PKG_VERSION"),
        repository = env!("CARGO_PKG_REPOSITORY"),
    );
}

fn call_rustc<I: Iterator<Item = String>>(rustc: &str, args: fn() -> I) -> Result<ExitCode> {
    // NOTE: not running inside Docker: local install should match Docker image setup
    let argz = || args().collect::<Vec<_>>();
    exit_code(
        Command::new(rustc)
            .args(args())
            .spawn()
            .with_context(|| format!("Failed to spawn rustc {rustc} with {:?}", argz()))?
            .wait()
            .with_context(|| format!("Failed to wait for rustc {rustc} with {:?}", argz()))?
            .code(),
    )
}

fn bake_rustc(
    crate_name: &str,
    arguments: Vec<String>,
    fallback: impl Fn() -> Result<ExitCode>,
) -> Result<ExitCode> {
    fn log_file() -> Result<File> {
        let log_path = log_path();
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("Failed opening (WA) log file {log_path}"))
    }

    if let Some(name) = env::var(RUSTCBUILDX_LOG_IF_CRATE_NAME).ok().as_deref() {
        if env::args().any(|arg| arg.contains(name)) {
            env::set_var(RUSTCBUILDX_LOG, "debug");
        }
    }

    if is_debug() {
        env_logger::Builder::from_env(
            Env::default().filter_or(RUSTCBUILDX_LOG, "debug").write_style(RUSTCBUILDX_LOG_STYLE),
        )
        .target(Target::Pipe(Box::new(log_file()?)))
        .init();
    }

    let krate = format!("{}:{crate_name}", env!("CARGO_PKG_NAME"));
    info!(target:&krate, "{bin}@{vsn} wraps `rustc` calls to BuildKit builders",
        bin = env!("CARGO_PKG_NAME"),
        vsn = env!("CARGO_PKG_VERSION"),
    );

    let pwd = env::current_dir().context("Failed to get $PWD")?;
    let pwd: Utf8PathBuf = pwd.try_into().context("Path's UTF-8 encoding is corrupted")?;

    let (st, args) = parse::as_rustc(&pwd, crate_name, arguments, false)?;
    info!(target:&krate, "{:?}", st);
    let RustcArgs { crate_type, emit, externs, metadata, incremental, input, out_dir, target_path } =
        st;

    let crate_out = env::var("OUT_DIR").ok().and_then(|x| x.ends_with("/out").then_some(x)); // NOTE: not `out_dir`

    let full_crate_id = format!("{crate_type}-{crate_name}-{metadata}");
    let krate = full_crate_id.as_str();

    // TODO: look into forwarding more envs: https://doc.rust-lang.org/nightly/cargo/reference/environment-variables.html
    env::vars().for_each(|(k, v)| debug!(target:&krate, "env is set: {k}={v:?}"));

    // https://github.com/rust-lang/cargo/issues/12059
    let mut all_externs = BTreeSet::new();
    let externs_prefix = |part: &str| Utf8Path::new(&target_path).join(format!("externs_{part}"));
    let crate_externs = externs_prefix(&format!("{crate_name}-{metadata}"));

    let ext = match crate_type.as_str() {
        "lib" => "rmeta".to_owned(),
        "bin" | "test" | "proc-macro" => "rlib".to_owned(),
        _ => bail!("BUG: unexpected crate-type: '{crate_type}'"),
    };
    // https://rustc-dev-guide.rust-lang.org/backend/libs-and-metadata.html#rmeta
    // > [rmeta] is created if the --emit=metadata CLI option is used.
    let ext = if emit.contains("metadata") { "rmeta".to_owned() } else { ext };

    if crate_type == "proc-macro" {
        // This way crates that depend on this know they must require it as .so
        let guard = format!("{crate_externs}_proc-macro");
        info!(target:&krate, "opening (RW) {guard}");
        fs::write(&guard, "").with_context(|| format!("Failed to `touch {guard}`"))?;
    };

    let mut short_externs = BTreeSet::new();
    for xtern in &externs {
        all_externs.insert(xtern.clone());

        if !xtern.starts_with("lib") {
            bail!("CONTRACT: cargo gave unexpected extern [^lib]: {xtern:?}")
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
        info!(target:&krate, "checking (RO) extern's externs {xtern_crate_externs}");
        if file_exists_and_is_not_empty(&xtern_crate_externs)
            .with_context(|| format!("Failed to `test -s {crate_externs}`"))?
        {
            info!(target:&krate, "opening (RO) crate externs {xtern_crate_externs}");
            let fd = File::open(&xtern_crate_externs)
                .with_context(|| format!("Failed to `cat {xtern_crate_externs}`"))?;
            for line in BufReader::new(fd).lines() {
                let transitive =
                    line.with_context(|| format!("Corrupted {xtern_crate_externs}"))?;
                assert_ne!(transitive, "");

                fn file_exists(path: impl AsRef<Utf8Path>) -> Result<bool> {
                    match path.as_ref().metadata().map(|md| md.is_file()) {
                        Ok(b) => Ok(b),
                        Err(e) => {
                            if e.kind() == ErrorKind::NotFound {
                                return Ok(false);
                            }
                            Err(e.into())
                        }
                    }
                }

                let guard = externs_prefix(&format!("{transitive}_proc-macro"));
                info!(target:&krate, "checking (RO) extern's guard {guard}");
                let actual_extern =
                    if file_exists(&guard).with_context(|| format!("Failed to `stat {guard}`"))? {
                        format!("lib{transitive}.so")
                    } else {
                        format!("lib{transitive}.{ext}")
                    };
                all_externs.insert(actual_extern.clone());

                // ^ this algo tried to "keep track" of actual paths to transitive deps artifacts
                //   however some edge cases (at least 1) go through. That fix seems to bust cache on 2nd builds though v

                if is_debug() {
                    let deps_dir = Utf8Path::new(&target_path).join("deps");
                    info!(target:&krate, "listing existing an extern crate's extern matches {deps_dir}/lib*.*");
                    let listing = read_dir(&deps_dir)
                        .with_context(|| format!("Failed reading directory {deps_dir}"))?
                        // TODO: at least context() error
                        .filter_map(std::result::Result::ok)
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
    info!(target:&krate, "checking (RO) externs {crate_externs}");
    if !file_exists_and_is_not_empty(&crate_externs)
        .with_context(|| format!("Failed to `test -s {crate_externs}`"))?
    {
        let mut shorts = String::new();
        for short_extern in &short_externs {
            shorts.push_str(short_extern);
            shorts.push('\n');
        }
        info!(target:&krate, "writing (RW) externs to {crate_externs}");
        fs::write(&crate_externs, shorts)
            .with_context(|| format!("Failed creating crate externs {crate_externs}"))?;
    }
    let all_externs = all_externs;
    info!(target:&krate, "crate_externs: {crate_externs}");
    debug!(target:&krate, "{crate_externs} = {data}", data = match read_to_string(&crate_externs) {
        Ok(data) => data,
        Err(e) => e.to_string(),
    });

    create_dir_all(&out_dir).with_context(|| format!("Failed to `mkdir -p {out_dir}`"))?;
    if let Some(ref incremental) = incremental {
        create_dir_all(incremental)
            .with_context(|| format!("Failed to `mkdir -p {incremental}`"))?;
    }

    let hm = |prefix: &str, basename: &str, pop: usize| {
        assert_eq!(pop, prefix.chars().filter(|c| *c == '_').count());
        let not_lowalnums = |c: char| {
            !("._-".contains(c) || c.is_ascii_digit() || (c.is_alphabetic() && c.is_lowercase()))
        };
        let basename = basename.replace(not_lowalnums, "_");
        (
            Some((format!("input_{prefix}--{basename}"), input.clone().popped(pop))),
            format!("{prefix}-{full_crate_id}"),
        )
    };

    let (input_mount, rustc_stage) = match input.iter().rev().take(4).collect::<Vec<_>>()[..] {
        ["lib.rs", "src"] => (None, format!("final-{full_crate_id}")),
        ["main.rs", "src"] => (None, format!("final-{full_crate_id}")),
        ["build.rs", "src", basename, ..] => hm("build__rs", basename, 2), // TODO: un-ducktape
        ["build.rs", basename, ..] => hm("build_rs", basename, 1),
        ["lib.rs", "src", basename, ..] => hm("src_lib_rs", basename, 2),
        // e.g. $HOME/.cargo/registry/src/github.com-1ecc6299db9ec823/fnv-1.0.7/lib.rs
        ["lib.rs", basename, ..] => hm("lib_rs", basename, 1),
        // e.g. $HOME/.cargo/registry/src/github.com-1ecc6299db9ec823/untrusted-0.7.1/src/untrusted.rs
        [rsfile, "src", basename, ..] if rsfile.ends_with(".rs") => hm("src__rs", basename, 2),
        // Running `CARGO=/home/runner/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo CARGO_CRATE_NAME=build_script_main CARGO_MANIFEST_DIR=/home/runner/.cargo/registry/src/index.crates.io-6f17d22bba15001f/openssl-sys-0.9.95 CARGO_PKG_AUTHORS='Alex Crichton <alex@alexcrichton.com>:Steven Fackler <sfackler@gmail.com>' CARGO_PKG_DESCRIPTION='FFI bindings to OpenSSL' CARGO_PKG_HOMEPAGE='' CARGO_PKG_LICENSE=MIT CARGO_PKG_LICENSE_FILE='' CARGO_PKG_NAME=openssl-sys CARGO_PKG_README=README.md CARGO_PKG_REPOSITORY='https://github.com/sfackler/rust-openssl' CARGO_PKG_RUST_VERSION='' CARGO_PKG_VERSION=0.9.95 CARGO_PKG_VERSION_MAJOR=0 CARGO_PKG_VERSION_MINOR=9 CARGO_PKG_VERSION_PATCH=95 CARGO_PKG_VERSION_PRE='' LD_LIBRARY_PATH='/home/runner/instst/release/deps:/home/runner/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib:/home/runner/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib' /home/runner/work/rustcbuildx/rustcbuildx/rustcbuildx /home/runner/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc --crate-name build_script_main --edition=2018 /home/runner/.cargo/registry/src/index.crates.io-6f17d22bba15001f/openssl-sys-0.9.95/build/main.rs --error-format=json --json=diagnostic-rendered-ansi,artifacts,future-incompat --crate-type bin --emit=dep-info,link -C embed-bitcode=no -C debug-assertions=off -C metadata=99f749eccead4467 -C extra-filename=-99f749eccead4467 --out-dir /home/runner/instst/release/build/openssl-sys-99f749eccead4467 -L dependency=/home/runner/instst/release/deps --extern cc=/home/runner/instst/release/deps/libcc-3c316ebdde73b0fe.rlib --extern pkg_config=/home/runner/instst/release/deps/libpkg_config-a6962381fee76247.rlib --extern vcpkg=/home/runner/instst/release/deps/libvcpkg-ebcbc23bfdf4209b.rlib --cap-lints warn`
        // /home/runner/.cargo/registry/src/index.crates.io-6f17d22bba15001f/openssl-sys-0.9.95/build/main.rs
        ["main.rs", "build", basename, ..] if crate_name == "build_script_main" => {
            // TODO: that's ducktape. Read Cargo.toml to match [package]build = "build/main.rs" ?
            // or just catchall >=4
            hm("main__rs", basename, 2)
        }
        _ => unreachable!("Unexpected input file {input:?}"),
    };
    info!(target:&krate, "picked {rustc_stage} for {suf:?}", suf=input.iter().rev().take(4).collect::<Vec<_>>());
    assert!(!matches!(input_mount, Some((_,ref x)) if x.ends_with("/.cargo/registry")));

    let incremental_stage = format!("incremental-{metadata}");
    let out_stage = format!("out-{metadata}");
    let stdio_stage = format!("stdio-{metadata}");
    // let mut toolchain = input_mount
    //     .as_ref()
    //     .map(|(_imn, imt)| -> Result<Option<String>> {
    //         let check = |file_name| -> Result<bool> {
    //             let p = Utf8Path::new(imt).join(file_name);
    //             info!(target:&krate, "checking (RO) toolchain file {p}");
    //             file_exists_and_is_not_empty(&p)
    //                 .with_context(|| format!("Failed to `test -s {p:?}`"))
    //         };
    //         for file_name in &["rust-toolchain.toml", "rust-toolchain"] {
    //             if check(file_name)? {
    //                 return Ok(Some(file_name.to_owned().to_owned()));
    //             }
    //         }
    //         Ok(None)
    //     })
    //     .transpose()?
    //     .flatten()
    //     .map(|toolchain_file_name|
    //         // https://rust-lang.github.io/rustup/overrides.html
    //         // NOTE: without this, the crate's rust-toolchain gets installed and used and (for the mentioned crate)
    //         //   fails due to (yet)unknown rustc CLI arg: `error: Unrecognized option: 'diagnostic-width'`
    //         // e.g. https://github.com/xacrimon/dashmap/blob/v5.4.0/rust-toolchain
    //         (format!("toolchain-{metadata}"), toolchain_file_name));
    // if true {
    //     // TODO: test building something involving rust-toolchain.toml
    //     toolchain = None;
    // }

    let mut dockerfile = String::new();

    // const RUSTUP_TOOLCHAIN: &str = "rustup-toolchain";
    // if let Some((stage, _)) = toolchain.as_ref() {
    //     writeln!(
    //         dockerfile,
    //         r#"FROM rust AS {stage}
    // RUN rustup default | cut -d- -f1 >/{RUSTUP_TOOLCHAIN}"#
    //     )?;
    // }

    writeln!(
        dockerfile,
        r#"FROM rust AS {rustc_stage}
WORKDIR {out_dir}"#
    )?;

    if let Some(incremental) = &incremental {
        writeln!(dockerfile, r#"WORKDIR {incremental}"#)?;
    }

    // https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates

    [
        "LD_LIBRARY_PATH", // TODO: see if that can be dropped
        "CARGO",
        "CARGO_MANIFEST_DIR",
        "CARGO_PKG_VERSION",
        "CARGO_PKG_VERSION_MAJOR",
        "CARGO_PKG_VERSION_MINOR",
        "CARGO_PKG_VERSION_PATCH",
        "CARGO_PKG_VERSION_PRE",
        "CARGO_PKG_AUTHORS",
        "CARGO_PKG_NAME",
        "CARGO_PKG_DESCRIPTION",
        "CARGO_PKG_HOMEPAGE",
        "CARGO_PKG_REPOSITORY",
        "CARGO_PKG_LICENSE",
        "CARGO_PKG_LICENSE_FILE",
        "CARGO_PKG_RUST_VERSION",
        "CARGO_CRATE_NAME",
        "CARGO_BIN_NAME",
        // TODO: allow additional envs to be passed as RUSTCBUILDX_ENV_* env(s)
        "OUT_DIR", // (Only set during compilation.)
    ]
    // TODO: CARGO_BIN_EXE_<name> — The absolute path to a binary target’s executable. This is only set when building an integration test or benchmark. This may be used with the env macro to find the executable to run for testing purposes. The <name> is the name of the binary target, exactly as-is. For example, CARGO_BIN_EXE_my-program for a binary named my-program. Binaries are automatically built when the test is built, unless the binary has required features that are not enabled.
    // TODO: CARGO_PRIMARY_PACKAGE — This environment variable will be set if the package being built is primary. Primary packages are the ones the user selected on the command-line, either with -p flags or the defaults based on the current directory and the default workspace members. This environment variable will not be set when building dependencies. This is only set when compiling the package (not when running binaries or tests).
    // TODO: CARGO_TARGET_TMPDIR — Only set when building integration test or benchmark code. This is a path to a directory inside the target directory where integration tests or benchmarks are free to put any data needed by the tests/benches. Cargo initially creates this directory but doesn’t manage its content in any way, this is the responsibility of the test code.
    .iter()
    .try_for_each(|var| -> Result<_> {
        let val = env::var(var)
            .ok()
            .and_then(|x| if x.is_empty() { None } else { Some(x) })
            .map(|x| format!("{x:?}"))
            .unwrap_or_default();
        writeln!(dockerfile, r#"ENV {var}={val}"#)?;
        Ok(())
    })?;

    let cwd = if let Some((name, target)) = input_mount.as_ref() {
        // Reuse previous contexts

        writeln!(
            dockerfile,
            // TODO: WORKDIR was removed as it changed during a single `cargo build`
            // Looks like removing it isn't an issue, however we need more testing.
            //             r#"WORKDIR {pwd}
            // RUN \
            //   --mount=type=bind,from={name},target={target} \"#
            r#"RUN \
  --mount=type=bind,from={name},target={target} \"#
        )?;

        // TODO: --build-arg BUILDKIT_CONTEXT_KEEP_GIT_DIR=0 https://docs.docker.com/engine/reference/builder/#buildkit-built-in-build-args

        None
    } else {
        // Save/send local workspace

        assert_eq!((input.is_relative(), input.as_str().ends_with(".rs")), (true, true));

        // TODO: try just bind mount instead of copying to a tmpdir
        // TODO: try not FWDing .git/* and equivalent
        // TODO: try filtering out CARGO_TARGET_DIR also
        // https://docs.docker.com/language/rust/develop/
        // RUN --mount=type=bind,source=src,target=src \
        //     --mount=type=bind,source=Cargo.toml,target=Cargo.toml \
        //     --mount=type=bind,source=Cargo.lock,target=Cargo.lock \

        let cwd = Temp::new_dir().context("Failed to create tmpdir 'cwd'")?;
        let Some(cwd_path) = Utf8Path::from_path(cwd.as_path()) else {
            bail!("Path's UTF-8 encoding is corrupted: {cwd:?}")
        };

        // TODO: use tmpfs when on *NIX
        // TODO: cache these folders
        if pwd.join(".git").is_dir() {
            info!(target:&krate, "copying all git files under {}", pwd.join(".git"));
            let output = Command::new("git")
                .arg("ls-files")
                .arg(&pwd)
                .output()
                .with_context(|| format!("Failed calling `git ls-files {pwd}`"))?;
            if !output.status.success() {
                bail!("Failed `git ls-files {pwd}`: {:?}", output.stderr)
            }
            // TODO: buffer reads to this command's output
            // NOTE: unsorted output lines
            for f in String::from_utf8(output.stdout).context("Parsing `git ls-files`")?.lines() {
                info!(target:&krate, "copying git repo file {f}");
                copy_file(Utf8Path::new(f), cwd_path)?;
            }
        } else {
            info!(target:&krate, "copying all files under {pwd}");
            copy_files(&pwd, cwd_path)?;
        }

        writeln!(
            dockerfile,
            r#"WORKDIR {pwd}
COPY --from=cwd / .
RUN \"#
        )?;

        Some(cwd)
    };

    if let Some(crate_out) = crate_out.as_ref() {
        writeln!(
            dockerfile,
            r#"  --mount=type=bind,from={named},target={crate_out} \"#,
            named = crate_out_name(crate_out)
        )?;
    }

    // if let Some((stage, _file_name)) = toolchain.as_ref() {
    //     writeln!(
    //         dockerfile,
    //         r#"  --mount=type=bind,from={stage},source=/{RUSTUP_TOOLCHAIN},target=/{RUSTUP_TOOLCHAIN} \"#
    //     )?;
    // }

    debug!(target:&krate, "all_externs = {all_externs:?}");
    assert!(externs.len() <= all_externs.len());
    let bakefiles = all_externs
        .into_iter()
        .map(|xtern| -> Result<_> {
            let Some((extern_bakefile, extern_bakefile_stage)) = bakefile_and_stage(xtern.clone(), &target_path) else {
                bail!("BUG: corrupted bakefile.hcl for {xtern}")
            };

            info!(target:&krate, "extern_bakefile: {extern_bakefile}");

            writeln!(dockerfile,
                    r#"  --mount=type=bind,from={extern_bakefile_stage},source=/{xtern},target={target_path}/deps/{xtern} \"#
            )?;

            Ok(extern_bakefile)
        })
        .collect::<Result<Vec<_>>>()?;

    // if toolchain.is_some() {
    //     writeln!(dockerfile, r#"    export RUSTUP_TOOLCHAIN="$(cat /{RUSTUP_TOOLCHAIN})" && \"#)?;
    //     // TODO: merge with iterator above
    // }

    // // https://rust-lang.github.io/rustup/overrides.html
    // // NOTE: without this, the crate's rust-toolchain gets installed and used.
    // // e.g. https://github.com/xacrimon/dashmap/blob/v5.4.0/rust-toolchain
    // // e.g. https://github.com/dtolnay/anyhow/blob/05e413219e97f101d8f39a90902e5c5d39f951fe/rust-toolchain.toml
    // // NOTE this is [[ -s "$input_mount_target"/rust-toolchain ]]
    // writeln!(
    //     dockerfile,
    //     //     r#"if [ -s ./rust-toolchain.toml ] || [ -s ./rust-toolchain ]; then \
    //     //     export RUSTUP_TOOLCHAIN="$(rustup default | cut -d- -f1)"; \
    //     // fi && \"#,
    //     r#"export RUSTUP_TOOLCHAIN=stable && \"#,
    // )?;

    const TMP_STDERR: &str = "stderr";
    const TMP_STDOUT: &str = "stdout";
    writeln!(
        dockerfile,
        r#"    if ! rustc '{args}' {input} >/{TMP_STDOUT} 2>/{TMP_STDERR}; then head /{TMP_STDOUT} /{TMP_STDERR}; exit 1; fi"#,
        args = args.join("' '"),
    )?;

    if let Some(incremental) = &incremental {
        writeln!(
            dockerfile,
            r#"FROM scratch AS {incremental_stage}
COPY --from={rustc_stage} {incremental} /"#,
        )?;
    }
    writeln!(
        dockerfile,
        r#"FROM scratch AS {stdio_stage}
COPY --from={rustc_stage} /{TMP_STDOUT} /{TMP_STDERR} /
FROM scratch AS {out_stage}
COPY --from={rustc_stage} {out_dir}/*-{metadata}* /"#,
    )?;
    // NOTE: -C extra-filename=-${metadata}

    let dockerfile = dockerfile; // Drop mut
    {
        let dockerfile_path = Utf8Path::new(&target_path).join(format!("{metadata}.Dockerfile"));
        info!(target:&krate, "opening (RW) crate dockerfile {dockerfile_path}");
        fs::write(&dockerfile_path, &dockerfile)
            .with_context(|| format!("Failed creating dockerfile {dockerfile_path}"))?;
    }

    let mut contexts: BTreeMap<_, _> = [
        Some(("rust".to_owned(), docker_image())),
        input_mount.map(|(name, target)| (name, target.to_string())),
        cwd.as_deref().map(|cwd| {
            let cwd_path = Utf8Path::from_path(cwd.as_path()).expect("PROOF: did not fail earlier");
            ("cwd".to_owned(), cwd_path.to_string())
        }),
        crate_out.as_deref().map(|crate_out| (crate_out_name(crate_out), crate_out.to_owned())),
    ]
    .into_iter()
    .flatten()
    .collect();

    // TODO: ask upstream `docker buildx bake` for a "dockerfiles" []string bake setting (that concatanates) or some way to inherit multiple dockerfiles (don't forget inlined ones)
    // TODO: ask upstream `docker buildx` for orderless stages (so we can concat Dockerfiles any which way, and save another DAG)

    let mut extern_dockerfiles: BTreeMap<_, _> = bakefiles
        .into_iter()
        .map(|extern_bakefile| -> Result<_> {
            info!(target:&krate, "opening (RO) extern bakefile {extern_bakefile}");
            let mounts = used_contexts(&extern_bakefile)?;
            let mounts_len = mounts.len();
            contexts.extend(mounts.into_iter());

            let extern_dockerfile = hcl_to_dockerfile(extern_bakefile);
            Ok((extern_dockerfile, mounts_len))
        })
        .collect::<Result<_>>()?;
    let mut dockerfile_bis = String::new();
    // Concat dockerfiles from topological sort of the DAG (stages must be defined first, then used)
    // Assumes that the more deps a crate has, the later it appears in the deps tree
    // TODO: do     vvvvvvvvv better than this
    for i_mounts in 0..999999usize {
        if extern_dockerfiles.is_empty() {
            break;
        }
        let matching: Vec<_> = extern_dockerfiles
            .iter()
            .filter(|(_, v)| **v == i_mounts)
            .map(|(k, _)| k)
            .cloned()
            .collect();
        for extern_dockerfile_path in matching {
            let res = extern_dockerfiles.remove(&extern_dockerfile_path);
            assert!(res.is_some());
            info!(target:&krate, "opening (RO) extern dockerfile {extern_dockerfile_path}");
            let extern_dockerfile = read_to_string(&extern_dockerfile_path)
                .with_context(|| format!("Failed reading dockerfile {extern_dockerfile_path}"))?;
            dockerfile_bis.push_str(extern_dockerfile.as_str());
            dockerfile_bis.push('\n');
        }
    }
    assert!(extern_dockerfiles.is_empty());
    dockerfile_bis.push_str(dockerfile.as_str());
    drop(dockerfile); // Earlier: wrote to disk

    let stdio = Temp::new_dir().context("Failed to create tmpdir 'stdio'")?;
    let Some(stdio_path) = Utf8Path::from_path(stdio.as_path()) else {
        bail!("Path's UTF-8 encoding is corrupted: {stdio:?}")
    };

    const TAB: char = '\t';
    let platform = "local".to_owned();
    let mut bakefile = String::new();

    writeln!(
        bakefile,
        r#"
target "{out_stage}" {{
{TAB}contexts = {{"#
    )?;
    let contexts: BTreeMap<_, _> = contexts.into_iter().collect();
    for (name, uri) in contexts {
        writeln!(bakefile, r#"{TAB}{TAB}"{name}" = "{uri}","#)?;
    }
    writeln!(
        bakefile,
        r#"{TAB}}}
{TAB}dockerfile-inline = <<DOCKERFILE
# syntax={docker_syntax}
{dockerfile_bis}
DOCKERFILE
{TAB}network = "none"
{TAB}output = ["{out_dir}"] # https://github.com/moby/buildkit/issues/1224
{TAB}platforms = ["{platform}"]
{TAB}target = "{out_stage}"
}}
target "{stdio_stage}" {{
{TAB}inherits = ["{out_stage}"]
{TAB}output = ["{stdio_path}"]
{TAB}target = "{stdio_stage}"
}}"#,
        docker_syntax = docker_syntax(),
    )?;

    let mut stages = vec![out_stage.as_str(), stdio_stage.as_str()];
    if let Some(incremental) = incremental.as_ref() {
        stages.push(incremental_stage.as_str());
        writeln!(
            bakefile,
            r#"
target "{incremental_stage}" {{
{TAB}inherits = ["{out_stage}"]
{TAB}output = ["{incremental}"]
{TAB}target = "{incremental_stage}"
}}"#,
        )?;
    }

    let bakefile_path = {
        let bakefile_path = format!("{target_path}/{crate_name}-{metadata}.hcl");
        info!(target:&krate, "opening (RW) crate bakefile {bakefile_path}");
        if is_debug() {
            match read_to_string(&bakefile_path) {
                Ok(existing) => {
                    let re = Regex::new(r#""\/tmp\/[^"]+""#)?;
                    let replacement = r#""REDACTED""#;
                    if false {
                        //FIXME
                        pretty_assertions::assert_eq!(
                            re.replace_all(&existing, replacement).to_string(),
                            re.replace_all(&bakefile, replacement).to_string(),
                        );
                    }
                }
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => bail!("{e}"),
            }
        }
        fs::write(&bakefile_path, bakefile)
            .with_context(|| format!("Failed creating bakefile {bakefile_path}"))?; // Don't remove HCL file
        bakefile_path
    };

    let mut cmd = Command::new("docker");
    if is_debug() {
        info!(target:&krate, "bakefile: {bakefile_path}");
        debug!(target:&krate, "{bakefile_path} = {data}", data = match read_to_string(&bakefile_path) {
            Ok(data) => data,
            Err(e) => e.to_string(),
        });
        cmd.arg("--debug").stdin(Stdio::null()).stdout(log_file()?).stderr(log_file()?);
    } else {
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    }
    cmd.arg("buildx").arg("bake").arg("--file").arg(&bakefile_path).args(stages);
    let start = Instant::now();
    let code = cmd
        .output()
        .with_context(|| format!("Failed calling `docker {args:?}`", args = cmd.get_args()))?
        .status
        .code();
    info!("command `docker buildx bake` ran in {}s: {code:?}", start.elapsed().as_secs());

    // TODO: buffered reading + copy to STDERR/STDOUT => give open fds in bakefile?
    for x in [true, false] {
        let path = stdio_path.join(if x { TMP_STDERR } else { TMP_STDOUT });
        info!(target:&krate, "reading (RO) {path}");
        let data = match read_to_string(&path) {
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            otherwise => otherwise,
        }
        .with_context(|| format!("Failed to copy {path}"))?;
        let msg = data.trim();
        debug!(target:&krate, "{path} ~= {msg}");
        if !msg.is_empty() {
            if x {
                eprintln!("{msg}");
            } else {
                println!("{msg}");
            }
        }
    }

    if !is_debug() {
        drop(stdio); // Removes stdio/std{err,out} files and stdio dir
        if let Some(cwd) = cwd {
            drop(cwd); // Removes tempdir contents
        }
    }

    if code != Some(0) {
        // TODO: re-enable
        if true {
            let _fallback = fallback;
            return exit_code(code);
        }
        // Bubble up actual error & outputs
        let res = fallback();
        error!(target:&krate, "A bug was found! {code:?}");
        eprintln!("Found a bug in this script!");
        return res;
    }

    exit_code(code)
}

fn exit_code(code: Option<i32>) -> Result<ExitCode> {
    // TODO: https://doc.rust-lang.org/std/os/unix/process/trait.ExitStatusExt.html
    Ok((code.unwrap_or(-1) as u8).into())
}

#[inline]
fn file_exists_and_is_not_empty(path: impl AsRef<Utf8Path>) -> Result<bool> {
    match path.as_ref().metadata().map(|md| md.is_file() && md.len() > 0) {
        Ok(b) => Ok(b),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

#[test]
fn fetches_back_used_contexts() {
    let tmp = Temp::new_file().unwrap();
    fs::write(&tmp, r#"
...
contexts = {
    "rust" = "docker-image://docker.io/library/rust:1.69.0-slim@sha256:8b85a8a6bf7ed968e24bab2eae6f390d2c9c8dbed791d3547fef584000f48f9e",
    "input_src_lib_rs--rustversion-1.0.9" = "/home/pete/.cargo/registry/src/github.com-1ecc6299db9ec823/rustversion-1.0.9",
    "crate_out-..." = "/home/pete/wefwefwef/network_products/ipam/ipam.git/target/debug/build/rustversion-ae69baa7face5565/out",
}
...
"#).unwrap();

    let exp=[
(    "input_src_lib_rs--rustversion-1.0.9".to_owned() , "/home/pete/.cargo/registry/src/github.com-1ecc6299db9ec823/rustversion-1.0.9".to_owned()),
 (   "crate_out-...".to_owned() , "/home/pete/wefwefwef/network_products/ipam/ipam.git/target/debug/build/rustversion-ae69baa7face5565/out".to_owned()),
        ].into_iter().collect();
    assert_eq!(used_contexts(Utf8Path::from_path(tmp.as_path()).unwrap()).unwrap(), exp);
}

fn used_contexts(path: impl AsRef<Utf8Path>) -> Result<BTreeMap<String, String>> {
    let path: &Utf8Path = path.as_ref();
    let fd = File::open(path).with_context(|| format!("Failed reading {path}"))?;
    BufReader::new(fd)
        .lines()
        .map_while(Result::ok)
        .filter(|line| {
            let ln = line.trim_start();
            ln.starts_with("\"input_") || ln.starts_with("\"crate_out-")
        })
        .map(|line| {
            if let [_, name, _, target, _] = line.splitn(5, '"').collect::<Vec<_>>()[..] {
                Ok((name.to_owned(), target.to_owned()))
            } else {
                bail!("corrupted extern_bakefile {path}: {line:?}")
            }
        })
        .collect::<Result<_>>()
}

#[test]
fn bakefile_and_stage_for_rlib() {
    let xtern = "libstrsim-8ed1051e7e58e636.rlib".to_owned();
    let res = bakefile_and_stage(xtern, "./target/path");
    assert_eq!(
        res,
        Some((
            "./target/path/strsim-8ed1051e7e58e636.hcl".to_owned().into(),
            "out-8ed1051e7e58e636".to_owned()
        ))
    );
}

fn bakefile_and_stage(
    xtern: String,
    target_path: impl AsRef<Utf8Path>,
) -> Option<(Utf8PathBuf, String)> {
    assert!(xtern.starts_with("lib")); // TODO: stop doing that (stripping ^lib)
    let bk = xtern.strip_prefix("lib").and_then(|x| x.split_once('.')).map(|(x, _)| x);
    let sg = bk.and_then(|x| x.split_once('-')).map(|(_, x)| x).map(|x| format!("out-{x}"));
    let bk = bk.map(|x| target_path.as_ref().join(format!("{x}.hcl")));
    bk.zip(sg)
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

#[test]
fn a_few_hcl_to_dockerfile() {
    assert_eq!(
        hcl_to_dockerfile("target/path/strsim-8ed1051e7e58e636.hcl".into()),
        "target/path/8ed1051e7e58e636.Dockerfile".to_owned()
    );
    assert_eq!(
        hcl_to_dockerfile("target/path/blip_blap-blop-1312051e7e58e636.hcl".into()),
        "target/path/1312051e7e58e636.Dockerfile".to_owned()
    );
}

fn hcl_to_dockerfile(mut hcl: Utf8PathBuf) -> Utf8PathBuf {
    let file_name = hcl
        .file_stem()
        .and_then(|x| x.rsplit_once('-').map(|(_, x)| x.to_owned()))
        .expect("PROOF: FIXME");
    let ok = hcl.pop();
    assert!(ok);
    hcl.join(format!("{file_name}.Dockerfile"))
}

fn copy_file(f: &Utf8Path, cwd: &Utf8Path) -> Result<()> {
    let Some(f_dirname) = f.parent() else { bail!("BUG: unexpected f={f:?} cwd={cwd:?}") };
    let dst = cwd.join(f_dirname);
    create_dir_all(&dst).with_context(|| format!("Failed `mkdir -p {dst}`"))?;
    let dst = cwd.join(f);
    fs::copy(f, &dst).with_context(|| format!("Failed `cp {f} {dst}`"))?;
    Ok(())
}

fn copy_files(dir: &Utf8Path, dst: &Utf8Path) -> Result<()> {
    if dir.is_dir() {
        // TODO: deterministic iteration
        for entry in read_dir(dir).with_context(|| format!("Failed reading dir {dir}"))? {
            let entry = entry?;
            let entry = entry.path();
            let entry = entry.as_path(); // thanks, Rust
            let Some(path) = Utf8Path::from_path(entry) else {
                bail!("Path's UTF-8 encoding is corrupted: {entry:?}")
            };
            if path.is_dir() {
                copy_files(path, dst)?;
            } else {
                copy_file(path, dst)?;
            }
        }
    }
    Ok(())
}
