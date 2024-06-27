//TODO: https://github.com/messense/cargo-options/blob/086d7470cae34b0e694a62237e258fbd35384e93/examples/cargo-mimic.rs
// maybe https://lib.rs/crates/clap-cargo

use std::{
    env,
    path::PathBuf,
    process::{Command, ExitCode, Stdio},
};

use anyhow::{anyhow, bail, Result};
use supergreen::{envs::runner, extensions::ShowCmd};

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

fn main() -> Result<ExitCode, Box<dyn std::error::Error>> {
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

    if let Err(e) = (|| -> Result<()> {
        let bin = ensure_binary_exists("rustcbuildx")?;

        setup_build_driver()?;

        // TODO pull-images
        // TODO read package.metadata.green
        // TODO: TUI above cargo output

        cmd.env("RUSTCBUILDX_LOG", env::var("RUSTCBUILDX_LOG").unwrap_or("debug".to_owned()));
        cmd.env(
            "RUSTCBUILDX_LOG_PATH",
            env::var("RUSTCBUILDX_LOG_PATH").unwrap_or("/tmp/cargo-green.log".to_owned()),
        );
        if let Ok(ctx) = env::var("RUSTCBUILDX_CACHE_IMAGE") {
            cmd.env("RUSTCBUILDX_CACHE_IMAGE", ctx);
        }
        if let Ok(wrapper) = env::var("RUSTC_WRAPPER") {
            bail!(
                r#"
    You called `cargo-green` but a $RUSTC_WRAPPER is already set (to {wrapper})
        We don't know what to do...
"#
            )
        }
        cmd.env("RUSTC_WRAPPER", bin);

        Ok(())
    })() {
        eprintln!("{e}");
        return Ok(ExitCode::FAILURE);
    }

    let status = cmd.status()?;
    Ok(status.code().map_or(ExitCode::FAILURE, |code| ExitCode::from(code as u8)))
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
fn setup_build_driver() -> Result<()> {
    let name = "supergreen";

    try_removing_previous_builder(name);

    let mut cmd = Command::new(runner());
    cmd.arg("--debug");
    cmd.args(["buildx", "create"]);
    cmd.arg(format!("--name={name}"));
    cmd.arg("--driver=docker-container");
    cmd.arg("--driver-opt=image=docker.io/moby/buildkit:buildx-stable-1@sha256:5d410bbb6d22b01fcaead1345936c5e0a0963eb6c3b190e38694e015467640fe");

    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());

    let call = cmd.show();
    let envs: Vec<_> = cmd.get_envs().map(|(k, v)| format!("{k:?}={v:?}")).collect();
    let envs = envs.join(" ");

    eprintln!("Calling {call} (env: {envs:?})`");

    if !cmd.status()?.success() {
        panic!("FIXME")
    }

    Ok(())
}

fn try_removing_previous_builder(name: &str) {
    let mut cmd = Command::new(runner());
    cmd.arg("--debug");
    cmd.args(["buildx", "rm", name]);

    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());

    let call = cmd.show();
    let envs: Vec<_> = cmd.get_envs().map(|(k, v)| format!("{k:?}={v:?}")).collect();
    let envs = envs.join(" ");

    eprintln!("Calling {call} (env: {envs:?})`");

    let _ = cmd.status();
}
