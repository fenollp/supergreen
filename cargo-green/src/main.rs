use std::{env, ffi::OsStr, path::PathBuf, process::exit};

use anyhow::{bail, Result};
use tokio::process::Command;

mod base;
mod cargo_green;
mod cratesio;
mod envs;
mod extensions;
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

    //TODO: https://github.com/messense/cargo-options/blob/086d7470cae34b0e694a62237e258fbd35384e93/examples/cargo-mimic.rs
    // maybe https://lib.rs/crates/clap-cargo

    match env::args().nth(3).as_deref() {
        None => {}
        Some("fetch") => {
            // TODO: run cargo fetch + read lockfile + generate cratesio stages + build them cacheonly
            //   https://github.com/rustsec/rustsec/tree/main/cargo-lock
            // TODO: skip these stages (and any other "locked thing" stage) when building with --no-cache
            todo!("BUG: this is never run") //=> fetch on first ever run!
        }
        Some(_) => {}
    }

    if !cmd.status().await?.success() {
        exit(1)
    }
    Ok(())
}
