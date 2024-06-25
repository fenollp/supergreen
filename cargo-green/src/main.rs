//TODO: https://github.com/messense/cargo-options/blob/086d7470cae34b0e694a62237e258fbd35384e93/examples/cargo-mimic.rs
// maybe https://lib.rs/crates/clap-cargo

use std::{
    env,
    path::PathBuf,
    process::{Command, ExitCode, Stdio},
};

use anyhow::{anyhow, bail, Result};
use supergreen::extensions::ShowCmd;

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

        // TODO pull-images
        // TODO read package.metadata.green
        // TODO: TUI above cargo output

        setup_build_driver()?;

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
    let mut cmd = /*tokio::process::*/Command::new("docker");
    cmd.arg("--debug");
    cmd.args(["buildx", "create"]);

    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());

    // Makes sure the underlying OS process dies with us
    // cmd.kill_on_drop(true);

    let call = cmd.show();
    let envs: Vec<_> = cmd.get_envs().map(|(k, v)| format!("{k:?}={v:?}")).collect();
    let envs = envs.join(" ");

    eprintln!("Starting {call} (env: {envs:?})`");

    let _code = cmd.status()?;
    //     .spawn()
    //     .map_err(|e| anyhow!("Failed to spawn {}: {e}", cmd.show()))?
    //     .wait()
    //     .await
    //     .map_err(|e| anyhow!("Failed to wait {}: {e}", cmd.show()))?
    //     .code();
    // Ok(exit_code(code))

    // let errf = |e| anyhow!("Failed starting {call}: {e}");
    // let mut child = cmd.spawn().map_err(errf)?;

    // let pid = child.id().unwrap_or_default();
    // log::info!(target: &krate, "Started {call} as pid={pid}`");
    // let krate = format!("{krate}@{pid}");

    // let out = TokioBufReader::new(child.stdout.take().expect("started")).lines();
    // let err = TokioBufReader::new(child.stderr.take().expect("started")).lines();

    // // TODO: try https://github.com/docker/buildx/pull/2500/files + find podman equivalent?
    // let out_task = fwd(krate.clone(), out, "stdout", "➤", MARK_STDOUT);
    // let err_task = fwd(krate.clone(), err, "stderr", "✖", MARK_STDERR);

    // let (secs, code) = {
    //     let start = Instant::now();
    //     let res = child.wait().await;
    //     let elapsed = start.elapsed();
    //     (elapsed, res.map_err(|e| anyhow!("Failed calling {call}: {e}"))?.code())
    // };
    // log::info!(target: &krate, "command `{command} build` ran in {secs:?}: {code:?}");

    // let longish = Duration::from_secs(2);
    // match join!(timeout(longish, out_task), timeout(longish, err_task)) {
    //     (Err(e), _) | (_, Err(e)) => panic!(">>> {krate} ({longish:?}): {e}"),
    //     (_, _) => {}
    // }
    // drop(child);

    // docker buildx create \
    //   --name supergreen \
    //   --driver=docker-container \
    //   --driver-opt=image=docker.io/moby/buildkit:buildx-stable-1@sha256:5d410bbb6d22b01fcaead1345936c5e0a0963eb6c3b190e38694e015467640fe
    // container

    Ok(())
}
