//TODO: https://github.com/messense/cargo-options/blob/086d7470cae34b0e694a62237e258fbd35384e93/examples/cargo-mimic.rs
// maybe https://lib.rs/crates/clap-cargo

use std::{
    env,
    ffi::OsStr,
    fs::OpenOptions,
    path::PathBuf,
    process::{ExitCode, Output, Stdio},
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
mod runner;
mod rustc_arguments;
mod stage;
mod wrap;

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
async fn main() -> Result<ExitCode> {
    let mut args = env::args();

    let arg0 = args.next().expect("$0 has to be set");
    if PathBuf::from(&arg0).file_name() != Some(OsStr::new(PKG)) {
        eprintln!(
            r#"
    This binary should be named `{PKG}`.
"#
        );
        return Ok(ExitCode::FAILURE);
    }

    if let Ok(wrapper) = env::var("RUSTC_WRAPPER") {
        if PathBuf::from(&wrapper).file_name() != Some(OsStr::new(PKG)) {
            eprintln!(
                r#"
    A $RUSTC_WRAPPER other than `{PKG}` is already set: {wrapper}.
"#
            );
            return Ok(ExitCode::FAILURE);
        }
        // Now running as a subprocess
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

    match env::args().nth(3).as_deref() {
        None => {}
        Some("fetch") => {
            // TODO: run cargo fetch + read lockfile + generate cratesio stages + build them cacheonly
            //   https://github.com/rustsec/rustsec/tree/main/cargo-lock
            // TODO: skip these stages (and any other "locked thing" stage) when building with --no-cache
            todo!("BUG: this is never run") //=> fetch on first ever run!
        }
        // TODO: Skip this call for the ones not calling rustc
        Some(_) => {
            cmd.env("RUSTC_WRAPPER", arg0);
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
    // TODO read package.metadata.green
    // https://lib.rs/crates/cargo_metadata
    // https://github.com/stormshield/cargo-ft/blob/d4ba5b048345ab4b21f7992cc6ed12afff7cc863/src/package/metadata.rs
    // TODO: TUI above cargo output (? https://docs.rs/prodash )

    if let Ok(log) = env::var("CARGOGREEN_LOG") {
        let mut val = String::new();
        for (var, def) in [
            (internal::RUSTCBUILDX_LOG, log),
            (internal::RUSTCBUILDX_LOG_PATH, "/tmp/cargo-green.log".to_owned()), // last
        ] {
            val = env::var(var).unwrap_or(def.to_owned());
            cmd.env(var, &val);
        }
        let _ = OpenOptions::new().create(true).truncate(false).append(true).open(val);
    }

    // RUSTCBUILDX is handled by `rustcbuildx`

    env::set_var("CARGOGREEN", "1");

    // Not calling envs::{syntax,base_image} directly so value isn't locked now.
    // Goal: produce only fully-locked Dockerfiles/TOMLs

    let mut syntax = internal::syntax().unwrap_or_else(|| DEFAULT_SYNTAX.to_owned());
    // Use local hashed image if one matching exists locally
    if !syntax.contains('@') {
        // otherwise default to a hash found through some Web API
        syntax = fetch_digest(&syntax).await?; //TODO: lower conn timeout to 4s (is 30s)
                                               //TODO: review online code to try to provide an offline mode
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

    // FIXME "multiplex conns to daemon" https://github.com/docker/buildx/issues/2564#issuecomment-2207435201
    // > If you do have docker context created already on ssh endpoint then you don't need to set the ssh address again on buildx create, you can use the context name or let it use the active context.

    // https://linuxhandbook.com/docker-remote-access/
    // https://thenewstack.io/connect-to-remote-docker-machines-with-docker-context/
    // https://www.cyberciti.biz/faq/linux-unix-reuse-openssh-connection/
    // https://github.com/moby/buildkit/issues/4268#issuecomment-2128464135
    // https://github.com/moby/buildkit/blob/v0.15.1/session/sshforward/sshprovider/agentprovider.go#L119

    // https://crates.io/crates/async-ssh2-tokio
    // https://crates.io/crates/russh

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

    let Output { status, stderr, .. } = cmd.output().await?;
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr);
        if !stderr.starts_with(r#"ERROR: existing instance for "supergreen""#) {
            bail!("BUG: failed to create builder: {stderr}")
        }
    }

    Ok(())
}

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
