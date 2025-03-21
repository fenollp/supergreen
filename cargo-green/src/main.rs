use std::{
    env,
    ffi::OsStr,
    fs::{self},
    path::PathBuf,
    process::exit,
};

use anyhow::{anyhow, bail, Result};
use camino::Utf8PathBuf;
use cargo_green::setup_build_driver;
use cratesio::add_step;
use envs::{builder_image, internal, runner, DEFAULT_SYNTAX};
use lockfile::{find_lockfile, locked_crates};
use runner::{build, fetch_digest, maybe_lock_image};
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

            let syntax = preset_syntax().await?;
            let syntax = syntax.trim_start_matches("docker-image://");
            preset_builder().await?;

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

async fn preset_syntax() -> Result<String> {
    let mut syntax = internal::syntax().unwrap_or_else(|| DEFAULT_SYNTAX.to_owned());
    // Use local hashed image if one matching exists locally
    if !syntax.contains('@') {
        // otherwise default to a hash found through some Web API
        syntax = fetch_digest(&syntax).await?; //TODO: lower conn timeout to 4s (is 30s)
                                               //TODO: review online code to try to provide an offline mode
    }
    Ok(syntax)
}

async fn preset_builder() -> Result<()> {
    let builder_image = builder_image().await;

    // https://docs.docker.com/build/building/variables/#buildx_builder
    if let Ok(ctx) = env::var("DOCKER_HOST") {
        eprintln!("$DOCKER_HOST is set to {ctx:?}");
    } else if let Ok(ctx) = env::var("BUILDX_BUILDER") {
        eprintln!("$BUILDX_BUILDER is set to {ctx:?}");
    } else if let Ok(remote) = env::var("CARGOGREEN_REMOTE") {
        //     // docker buildx create \
        //     //   --name supergreen \
        //     //   --driver remote \
        //     //   tcp://localhost:1234
        //     //{remote}
        //     env::set_var("DOCKER_CONTEXT", "supergreen"); //FIXME: ensure this gets passed down & used
        panic!("$CARGOGREEN_REMOTE is reserved but set to: {remote}");
    } else if false {
        // Images were pulled, we have to re-read their now-locked values now
        let builder_image = maybe_lock_image(builder_image.to_owned()).await;
        setup_build_driver("supergreen", builder_image.trim_start_matches("docker-image://"))
            .await?; // FIXME? maybe_..
        env::set_var("BUILDX_BUILDER", "supergreen");

        // TODO? docker dial-stdio proxy
        // https://github.com/docker/cli/blob/9bb1a62735174e9220d84fecc056a0ef8a1fc26f/cli/command/system/dial_stdio.go

        // https://docs.docker.com/engine/context/working-with-contexts/
        // https://docs.docker.com/engine/security/protect-access/
    }
    Ok(())
}
