//TODO: https://github.com/messense/cargo-options/blob/086d7470cae34b0e694a62237e258fbd35384e93/examples/cargo-mimic.rs
// maybe https://lib.rs/crates/clap-cargo

use std::{
    env,
    ffi::OsStr,
    fs::OpenOptions,
    path::PathBuf,
    process::{ExitCode, Stdio},
};

use anyhow::{bail, Result};
use tokio::process::Command;

use crate::{
    base::BaseImage,
    cli::{envs, help, pull, push},
    envs::{builder_image, cache_image, incremental, internal, runner, DEFAULT_SYNTAX},
    extensions::ShowCmd,
    runner::{fetch_digest, maybe_lock_image, runner_cmd},
    wrap::do_wrap,
};

mod base;
mod cli;
mod cratesio;
mod envs;
mod extensions;
mod md;
mod parse;
mod runner;
mod stage;
mod wrap;

const PKG: &str = env!("CARGO_PKG_NAME");
const REPO: &str = env!("CARGO_PKG_REPOSITORY");
const VSN: &str = env!("CARGO_PKG_VERSION");

/*

\cargo green +nightly fmt --all

\cargo green clippy --locked --frozen --offline --all-targets --all-features -- -D warnings --no-deps -W clippy::cast_lossless -W clippy::redundant_closure_for_method_calls -W clippy::str_to_string

\cargo green auditable build --locked --frozen --offline --all-targets --all-features

\cargo green test --all-targets --all-features --locked --frozen --offline

\cargo green nextest run --all-targets --all-features --locked --frozen --offline

*/

//TODO test
// \cargo green +nightly --version # check displayed vsn
// \cargo green --version # check != displayed vsn
// \cargo green # check displays help

#[tokio::main]
async fn main() -> Result<ExitCode> {
    let mut args = env::args();
    let arg0 = args.next().expect("$0 has to be set");
    if let Ok(wrapper) = env::var("RUSTC_WRAPPER") {
        assert_eq!(PathBuf::from(arg0).file_name(), Some(OsStr::new(env!("CARGO_PKG_NAME")))); // FIXME

        if false {
            panic!(
                ">>> {wrapper:?}\n{:?}\n{:?}",
                env::args().collect::<Vec<_>>(),
                env::vars().collect::<Vec<_>>()
            )
        }

        return Ok(do_wrap().await);
    }

    let Some(arg1) = args.next() else {
        // Warn when running this via `cargo run -p cargo-green -- ..`
        eprintln!(
            r#"
    The `{PKG}` binary needs to be called via cargo, e.g.:
        cargo green build
"#
        );
        return Ok(ExitCode::FAILURE);
    };
    if ["-h", "--help", "-V", "--version"].contains(&arg1.as_str()) {
        return Ok(help());
    }
    assert_eq!(arg1.as_str(), "green");

    let mut cmd = Command::new(env::var("CARGO").unwrap_or("cargo".into()));
    if let Some(arg) = args.next() {
        if arg == "supergreen" {
            return match args.next().as_deref() {
                None | Some("-h" | "--help" | "-V" | "--version") => Ok(help()),
                Some("env") => Ok(envs(args.collect()).await),
                Some("pull") => pull().await,
                Some("push") => push().await,
                Some(arg) => {
                    eprintln!("Unexpected supergreen command {arg:?}");
                    return Ok(ExitCode::FAILURE);
                }
            };
        }
        cmd.arg(&arg);
        if arg.starts_with('+') {
            cmd = Command::new(which::which("cargo").unwrap_or("cargo".into()));
            eprintln!("Passing +toolchain param: {arg:?}");
            // TODO: reinterpret `rustc` when given `+toolchain`
        }
    }
    cmd.args(args);
    cmd.kill_on_drop(true);

    match env::args().nth(3).as_deref() {
        None => {}
        Some("fetch") => {
            // TODO: run cargo fetch + read lockfile + generate cratesio stages + build them cacheonly
            //   https://github.com/rustsec/rustsec/tree/main/cargo-lock
            // TODO: skip these stages (and any other "locked thing" stage) when building with --no-cache
        }
        // TODO: Skip this call for the ones not calling rustc
        Some(_) => {
            if let Err(e) = setup_for_build(&mut cmd).await {
                eprintln!("{e}");
                return Ok(ExitCode::FAILURE);
            }
        }
    }

    let status = cmd.status().await?;
    Ok(status.code().map_or(ExitCode::FAILURE, |code| ExitCode::from(code as u8)))
}

async fn setup_for_build(cmd: &mut Command) -> Result<()> {
    if let Ok(wrapper) = env::var("RUSTC_WRAPPER") {
        bail!(
            r#"
    You called `{PKG}` but a $RUSTC_WRAPPER is already set (to {wrapper})
        We don't know what to do...
"#
        )
    }
    cmd.env("RUSTC_WRAPPER", env::args().next().expect("$0 has to be set"));

    // TODO read package.metadata.green
    // TODO: TUI above cargo output (? https://docs.rs/prodash )

    if let Ok(log) = env::var("CARGOGREEN_LOG") {
        let path = "/tmp/cargo-green.log";
        for (var, def) in [
            //
            (internal::RUSTCBUILDX_LOG, log),
            (internal::RUSTCBUILDX_LOG_PATH, path.to_owned()),
        ] {
            cmd.env(var, env::var(var).unwrap_or(def.to_owned()));
            let _ = OpenOptions::new().create(true).truncate(false).append(true).open(path);
        }
    }

    // RUSTCBUILDX is handled by `rustcbuildx`

    env::set_var("CARGOGREEN", "1");

    // Not calling envs::{syntax,base_image} directly so value isn't locked now.
    // Goal: produce only fully-locked Dockerfiles/TOMLs

    let mut syntax = internal::syntax().unwrap_or_else(|| DEFAULT_SYNTAX.to_owned());
    // Use local hashed image if one matching exists locally
    if !syntax.contains('@') {
        // otherwise default to a hash found through some Web API
        syntax = fetch_digest(&syntax).await?;
    }
    env::set_var(internal::RUSTCBUILDX_SYNTAX, syntax);

    let mut base_image = BaseImage::from_rustc_v()?.maybe_lock_base().await;
    let base = base_image.base();
    if !base.contains('@') {
        base_image = base_image.lock_base_to(fetch_digest(&base).await?);
    }
    env::set_var(internal::RUSTCBUILDX_BASE_IMAGE, base_image.base());

    let base_image_block = base_image.block();
    env::set_var("RUSTCBUILDX_BASE_IMAGE_BLOCK_", base_image_block.clone());
    cmd.env(
        internal::RUSTCBUILDX_RUNS_ON_NETWORK,
        internal::runs_on_network().unwrap_or_else(|| {
            if base_image_block.contains(" apt-get ") { "default" } else { "none" }.to_owned()
        }),
    );

    let builder_image = builder_image().await;

    // don't pull
    if let Some(val) = cache_image() {
        cmd.env(internal::RUSTCBUILDX_CACHE_IMAGE, val);
    }

    if incremental() {
        cmd.env(internal::RUSTCBUILDX_INCREMENTAL, "1");
    }
    // RUSTCBUILDX_LOG
    // RUSTCBUILDX_LOG_PATH
    // RUSTCBUILDX_LOG_STYLE
    cmd.env(internal::RUSTCBUILDX_RUNNER, runner());

    // FIXME https://github.com/docker/buildx/issues/2564
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

// https://docs.docker.com/build/drivers/docker-container/
// https://docs.docker.com/build/drivers/remote/
// https://docs.docker.com/build/drivers/kubernetes/
async fn setup_build_driver(name: &str, builder_image: &str) -> Result<()> {
    if false {
        // TODO: reuse old state but try auto-upgrading builder impl
        try_removing_previous_builder(name).await;
    }

    let mut cmd = runner_cmd();
    cmd.args(["buildx", "create"])
        .arg(format!("--name={name}"))
        .arg("--bootstrap")
        .arg("--driver=docker-container")
        .arg(format!("--driver-opt=image={builder_image}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let call = cmd.show();
    let envs: Vec<_> = cmd.as_std().get_envs().map(|(k, v)| format!("{k:?}={v:?}")).collect();
    let envs = envs.join(" ");

    eprintln!("Calling {call} (env: {envs:?})`");

    let res = cmd.output().await?;
    if !res.status.success() {
        let stderr = String::from_utf8(res.stderr)?;
        if !stderr.starts_with(r#"ERROR: existing instance for "supergreen""#) {
            bail!("BUG: failed to create builder: {stderr}")
        }
    }

    Ok(())
}

#[allow(dead_code)]
async fn try_removing_previous_builder(name: &str) {
    let mut cmd = runner_cmd();
    cmd.args(["buildx", "rm", name, "--keep-state", "--force"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let call = cmd.show();
    let envs: Vec<_> = cmd.as_std().get_envs().map(|(k, v)| format!("{k:?}={v:?}")).collect();
    let envs = envs.join(" ");

    eprintln!("Calling {call} (env: {envs:?})`");

    let _ = cmd.status().await;
}
