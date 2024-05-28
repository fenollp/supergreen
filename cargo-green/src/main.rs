//TODO: https://github.com/messense/cargo-options/blob/086d7470cae34b0e694a62237e258fbd35384e93/examples/cargo-mimic.rs
// maybe https://lib.rs/crates/clap-cargo

use std::{
    env,
    ffi::OsString,
    path::PathBuf,
    process::{Command as StdCommand, ExitCode},
};

use anyhow::{anyhow, Result};

fn main() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let cargo = env::var("CARGO").unwrap_or("cargo".into());
    let args: Vec<OsString> = env::args_os().skip(2).collect(); // skips $0 and "green"

    let mut cmd = StdCommand::new(cargo);

    if ["build".into(), "test".into(), "supergreen".into() /*FIXME*/].contains(&args[0]) {
        if let Err(e) = (|| -> Result<()> {
            let bin = ensure_binary_exists("rustcbuildx")?;

            // TODO pull-images

            cmd.env("RUSTCBUILDX_LOG", "debug");
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
    }

    cmd.args(args);
    eprintln!(">>> {:?}", cmd.get_args());
    eprintln!(">>> {:?}", cmd.get_envs());
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
