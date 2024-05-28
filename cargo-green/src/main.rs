//TODO: https://github.com/messense/cargo-options/blob/086d7470cae34b0e694a62237e258fbd35384e93/examples/cargo-mimic.rs
// maybe https://lib.rs/crates/clap-cargo

use std::{
    env,
    ffi::OsString,
    path::PathBuf,
    process::{Command as StdCommand, ExitCode},
};

use anyhow::{anyhow, Result};

/*

\cargo green +nightly fmt --all

\cargo green clippy --locked --frozen --offline --all-targets --all-features -- -D warnings --no-deps -W clippy::cast_lossless -W clippy::redundant_closure_for_method_calls -W clippy::str_to_string

\cargo green auditable build --locked --frozen --offline --all-targets --all-features

\cargo green test --all-targets --all-features --locked --frozen --offline

\cargo green nextest run --all-targets --all-features --locked --frozen --offline

*/

fn main() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let cargo = env::var("CARGO").unwrap_or("cargo".into());
    let mut args: Vec<OsString> = env::args_os().skip(2).collect(); // skips $0 and "green"

    let mut cmd = StdCommand::new(cargo);

    // Rewrites `cargo green +nightly ..` into `cargo +nightly green ..`
    if args[0].as_encoded_bytes().starts_with(&[b'+']) {
        cmd.arg(&args[0]);
        args = args.into_iter().skip(1).collect();
    }

    if args[0] == *"supergreen" {
        // FIXME: handle commands
    }

    // if [
    //     // All cargo-provided subcommands
    //     "add".into(),
    //     "b".into(),
    //     "bench".into(),
    //     "build".into(),
    //     "c".into(),
    //     "check".into(),
    //     "clean".into(),
    //     "clippy".into(),
    //     "d".into(),
    //     "doc".into(),
    //     "init".into(),
    //     "install".into(),
    //     "new".into(),
    //     "publish".into(),
    //     "r".into(),
    //     "remove".into(),
    //     "run".into(),
    //     "search".into(),
    //     "t".into(),
    //     "test".into(),
    //     "uninstall".into(),
    //     "update".into(),
    //     //
    //     "supergreen".into(), // Our subcommand
    // ]
    // .contains(&args[0])
    // {
    if let Err(e) = (|| -> Result<()> {
        let bin = ensure_binary_exists("rustcbuildx")?;

        // TODO pull-images

        // cmd.env("RUSTCBUILDX_LOG", "debug");
        cmd.env("RUSTCBUILDX_LOG", "info");
        cmd.env("RUSTCBUILDX_LOG_PATH", "/tmp/cargo-green.log"); // TODO: TUI above cargo output
        if let Ok(ctx) = env::var("RUSTCBUILDX_CACHE_IMAGE") {
            cmd.env("RUSTCBUILDX_CACHE_IMAGE", ctx);
        }
        cmd.env("RUSTC_WRAPPER", bin);

        Ok(())
    })() {
        eprintln!("{e}");
        return Ok(ExitCode::FAILURE);
    }
    // }

    cmd.args(args);
    eprintln!(">>> {:?}", cmd.get_args()); // FIXME: drop
    eprintln!(">>> {:?}", cmd.get_envs()); // FIXME: drop
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
