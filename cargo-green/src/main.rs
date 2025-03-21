use std::{
    env,
    ffi::OsStr,
    fs::{self},
    path::PathBuf,
    process::exit,
};

use anyhow::{anyhow, bail, Result};
use camino::Utf8PathBuf;
use cratesio::add_step;
use envs::{internal, runner};
use lockfile::{find_lockfile, locked_crates};
use runner::build;
use stage::Stage;
use tokio::process::Command;

mod base;
mod cargo_green;
mod cratesio;
mod envs;
mod extensions;
mod lockfile;
mod logging;
mod md;
mod runner;
mod rustc_arguments;
mod rustc_wrapper;
mod stage;
mod supergreen;

const PKG: &str = env!("CARGO_PKG_NAME");
const REPO: &str = env!("CARGO_PKG_REPOSITORY");
const VSN: &str = env!("CARGO_PKG_VERSION");

/*

\cargo green +nightly fmt --all
\cargo +nightly green fmt --all

\cargo green clippy --locked --frozen --offline --all-targets --all-features

\cargo green auditable build --locked --frozen --offline --all-targets --all-features
\cargo auditable green build --locked --frozen --offline --all-targets --all-features

\cargo green test --all-targets --all-features --locked --frozen --offline

\cargo green nextest run --all-targets --all-features --locked --frozen --offline

*/

//TODO test
// \cargo green +nightly --version # check displayed vsn
// \cargo green --version # check != displayed vsn
// \cargo green # check displays help

cargo_subcommand_metadata::description!(env!("CARGO_PKG_DESCRIPTION"));

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = env::args();

    let arg0 = args.next().expect("$0 has to be set");
    if PathBuf::from(&arg0).file_name() != Some(OsStr::new(PKG)) {
        bail!("This binary should be named `{PKG}`")
    }

    if let Ok(wrapper) = env::var("RUSTC_WRAPPER") {
        if PathBuf::from(&wrapper).file_name() != Some(OsStr::new(PKG)) {
            bail!("A $RUSTC_WRAPPER other than `{PKG}` is already set: {wrapper}")
        }
        // Now running as a subprocess

        let arg0 = env::args().nth(1);
        let args = env::args().skip(1).collect();
        let vars = env::vars().collect();
        return rustc_wrapper::main(arg0, args, vars).await;
    }

    if args.next().as_deref() != Some("green") {
        supergreen::help()?;
        exit(1)
    }

    let mut cmd = Command::new(env::var("CARGO").unwrap_or("cargo".into()));
    if let Some(arg) = args.next() {
        if arg == "supergreen" {
            return supergreen::main(args.next().as_deref(), args.collect()).await;
        }

        // https://rust-lang.github.io/rustup/overrides.html#toolchain-override-shorthand
        if let Some(toolchain) = arg.strip_prefix('+') {
            let var = "RUSTUP_TOOLCHAIN";
            if let Ok(val) = env::var(var) {
                println!("Overriding {var}={val:?} to {toolchain:?} for `{PKG} +toolchain`");
            }
            // Special handling: call was `cargo green +toolchain ..` (probably from `alias cargo='cargo green'`).
            // Normally, calls look like `cargo +toolchain green ..` but let's simplify alias creation!
            env::set_var(var, toolchain); // Informs `rustc -vV` when deciding base_image()
        } else {
            cmd.arg(&arg);
        }
    }
    cmd.args(args);
    cmd.kill_on_drop(true);

    // TODO: Skip this for the invocations not calling rustc
    cmd.env("RUSTC_WRAPPER", arg0);
    cargo_green::main(&mut cmd).await?;

    let manifest_path_lockfile = find_lockfile().await?;
    dbg!(&manifest_path_lockfile);
    cmd.env("CARGOGREEN_LOCKFILE", &manifest_path_lockfile);

    dbg!(std::env::vars());
    dbg!(&std::env::args());
    // [cargo-green/src/main.rs:116:5] &std::env::args() = Args { inner: [                                                                                                                                                                                          "/tmp/cargo-green/bin/cargo-green",                                                                                                                                                           "green",                                                                                                                                                                                      "-vv",                                                                                                                                                                                        "install",
    //     "--timings",
    //     "--jobs=1",
    //     "--root=/tmp",
    //     "--locked",
    //     "--force",
    //     "buildxargs",
    //     "--git",
    //     "https://github.com/fenollp/buildxargs.git",

    //=> cargo install can't find lockfile
    // => issue `cargo` to cp lockfile in $CARGO_TARGET_DIR?
    // => parse its args, understand source, guess lockfile?
    //https://github.com/rust-lang/cargo/issues/9700

    //TODO: https://github.com/messense/cargo-options/blob/086d7470cae34b0e694a62237e258fbd35384e93/examples/cargo-mimic.rs
    // maybe https://lib.rs/crates/clap-cargo

    match env::args().nth(2).as_deref() {
        None => {}
        Some("fetch") => {
            // First, run actuall `cargo fetch`
            if !cmd.status().await?.success() {
                exit(1)
            }

            logging::setup("fetch", internal::RUSTCBUILDX_LOG, internal::RUSTCBUILDX_LOG_STYLE);

            // TODO: skip these stages (and any other "locked thing" stage) when building with --no-cache

            let packages = locked_crates(&manifest_path_lockfile).await?;
            if packages.is_empty() {
                return Ok(());
            }

            let syntax = cmd
                .as_std()
                .get_envs()
                .filter_map(|(k, v)| (k == internal::RUSTCBUILDX_SYNTAX).then_some(v))
                .next()
                .flatten()
                .and_then(|x| x.to_str())
                .unwrap()
                .trim_start_matches("docker-image://");

            let mut dockerfile = format!("# syntax={syntax}\n");
            let stager = |i| format!("cargo-fetch-{i}");
            let mut leaves = 0;
            for (i, pkgs) in packages.chunks(127).enumerate() {
                leaves = i;
                dockerfile.push_str(&format!("FROM scratch AS {}\n", stager(i)));
                let (name, version, hash) = pkgs[0].clone();
                dockerfile.push_str(&add_step(&name, &version, &hash));
                for (name, version, hash) in &pkgs[1..] {
                    dockerfile.push_str(&add_step(name, version, hash));
                }
            }
            let stage = Stage::try_new("cargo-fetch")?;
            dockerfile.push_str(&format!("FROM scratch AS {stage}\n"));
            for leaf in 0..=leaves {
                dockerfile.push_str(&format!("COPY --from={} / /\n", stager(leaf)));
            }

            let cfetch: Utf8PathBuf = env::temp_dir().join("cargo-fetch").try_into()?;
            fs::create_dir_all(&cfetch)
                .map_err(|e| anyhow!("Failed `mkdir -p {cfetch:?}`: {e}"))?;

            let dockerfile_path = cfetch.join("Dockerfile");
            fs::write(&dockerfile_path, dockerfile)
                .map_err(|e| anyhow!("Failed creating dockerfile {dockerfile_path}: {e}"))?;

            // TOOD: test in CI + hack/

            return build(runner(), &dockerfile_path, stage, &[].into(), &cfetch).await;
        }
        Some(_) => {}
    }

    if !cmd.status().await?.success() {
        exit(1)
    }
    Ok(())
}
