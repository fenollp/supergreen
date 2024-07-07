//TODO: https://github.com/messense/cargo-options/blob/086d7470cae34b0e694a62237e258fbd35384e93/examples/cargo-mimic.rs
// maybe https://lib.rs/crates/clap-cargo

use std::{
    env,
    path::PathBuf,
    process::{ExitCode, Stdio},
};

use anyhow::{anyhow, bail, Result};
use supergreen::{
    base::BaseImage,
    envs::{base_image, builder_image, cache_image, incremental, internal, runner, syntax},
    extensions::ShowCmd,
};
use tokio::process::Command;

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
async fn main() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1); // skips $0

    // skips "green"
    if args.next().is_none() {
        // Warn when running this via `cargo run -p cargo-green -- ..`
        eprintln!(
            r#"
    The `cargo-green` binary needs to be called via cargo, e.g.:
        cargo green build
"#
        );
        return Ok(ExitCode::FAILURE);
    }

    // FIXME: make sure this handles cargo plugins
    let mut cmd = Command::new(env::var("CARGO").unwrap_or("cargo".into()));
    if let Some(arg) = args.next().as_deref() {
        cmd.arg(arg);
        if arg == "supergreen" {
            // FIXME: handle commands
        }
        if arg.starts_with('+') {
            cmd = Command::new(which::which("cargo").unwrap_or("cargo".into()));
            cmd.arg(arg);
            // TODO: reinterpret `rustc` when given `+toolchain`
        }
    }
    cmd.args(args);
    cmd.kill_on_drop(true);

    if let Err(e) = build(&mut cmd).await {
        eprintln!("{e}");
        return Ok(ExitCode::FAILURE);
    }

    let status = cmd.status().await?;
    Ok(status.code().map_or(ExitCode::FAILURE, |code| ExitCode::from(code as u8)))
}

async fn build(cmd: &mut Command) -> Result<()> {
    if let Ok(wrapper) = env::var("RUSTC_WRAPPER") {
        bail!(
            r#"
    You called `cargo-green` but a $RUSTC_WRAPPER is already set (to {wrapper})
        We don't know what to do...
"#
        )
    }
    cmd.env("RUSTC_WRAPPER", ensure_binary_exists("rustcbuildx")?);

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
        setup_build_driver("supergreen").await?; // FIXME? maybe_..
        env::set_var("BUILDX_BUILDER", "supergreen");
    }

    // TODO read package.metadata.green
    // TODO: TUI above cargo output (? https://docs.rs/prodash )

    if let Ok(log) = env::var("CARGOGREEN_LOG") {
        for (var, def) in [
            //
            (internal::RUSTCBUILDX_LOG, log),
            (internal::RUSTCBUILDX_LOG_PATH, "/tmp/cargo-green.log".to_owned()),
        ] {
            cmd.env(var, env::var(var).unwrap_or(def.to_owned()));
        }
    }

    // RUSTCBUILDX is handled by `rustcbuildx`
    // TODO? set a CARGOGREEN=1

    let base_image = base_image().await;
    env::set_var("RUSTCBUILDX_BASE_IMAGE_BLOCK_", base_image.block());
    if let Some(val) = internal::runs_on_network() {
        cmd.env(internal::RUSTCBUILDX_RUNS_ON_NETWORK, val);
    } else {
        cmd.env(
            internal::RUSTCBUILDX_RUNS_ON_NETWORK,
            if matches!(base_image, BaseImage::Image(_)) { "none" } else { "" },
        );
    }

    cmd.env(internal::RUSTCBUILDX_BUILDER_IMAGE, builder_image().await);
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
    cmd.env(internal::RUSTCBUILDX_SYNTAX, syntax().await);

    if let Some(val) = cache_image() {
        cmd.env(internal::RUSTCBUILDX_CACHE_IMAGE, val);
    }

    Ok(())
}

fn ensure_binary_exists(name: &'static str) -> Result<PathBuf> {
    which::which(name).map_err(|_| {
        anyhow!(
            r#"
    You called `cargo-green` but its dependency `rustcbuildx` cannot be found.
    Please run:
        # \cargo install --locked rustcbuildx
"#
        )
    })
}

// https://docs.docker.com/build/drivers/docker-container/
// https://docs.docker.com/build/drivers/remote/
// https://docs.docker.com/build/drivers/kubernetes/
async fn setup_build_driver(name: &str) -> Result<()> {
    if false {
        // TODO: reuse old state but try auto-upgrading builder impl
        try_removing_previous_builder(name).await;
    }

    let mut cmd = Command::new(runner());
    cmd.arg("--debug");
    cmd.args(["buildx", "create"]);
    cmd.arg(format!("--name={name}"));
    cmd.arg("--bootstrap");
    cmd.arg("--driver=docker-container");
    let img = builder_image().await.trim_start_matches("docker-image://");
    cmd.arg(&format!("--driver-opt=image={img}"));

    cmd.kill_on_drop(true);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());

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
    let mut cmd = Command::new(runner());
    cmd.arg("--debug");
    cmd.args(["buildx", "rm", name, "--keep-state", "--force"]);

    cmd.kill_on_drop(true);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());

    let call = cmd.show();
    let envs: Vec<_> = cmd.as_std().get_envs().map(|(k, v)| format!("{k:?}={v:?}")).collect();
    let envs = envs.join(" ");

    eprintln!("Calling {call} (env: {envs:?})`");

    let _ = cmd.status().await;
}
