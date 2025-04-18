use std::{
    env,
    ffi::{OsStr, OsString},
    path::PathBuf,
    process::exit,
};

use anyhow::{anyhow, bail, Result};
use tokio::process::Command;

mod base;
mod cargo_green;
mod checkouts;
mod cratesio;
mod envs;
mod extensions;
mod green;
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

fn cargo() -> OsString {
    env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = env::args();

    let arg0 = args.next().expect("$0 has to be set");
    if PathBuf::from(&arg0).file_name() != Some(OsStr::new(PKG)) {
        bail!("This binary should be named `{PKG}`")
    }

    const ENV_ROOT_PACKAGE_SETTINGS: &str = "CARGOGREEN_ROOT_PACKAGE_SETTINGS_";

    if let Ok(wrapper) = env::var("RUSTC_WRAPPER") {
        if PathBuf::from(&wrapper).file_name() != Some(OsStr::new(PKG)) {
            bail!("A $RUSTC_WRAPPER other than `{PKG}` is already set: {wrapper}")
        }
        // Now running as a subprocess

        let green = env::var(ENV_ROOT_PACKAGE_SETTINGS)
            .map_err(|_| anyhow!("BUG: ${ENV_ROOT_PACKAGE_SETTINGS} is unset"))?;
        let green = serde_json::from_str(&green)
            .map_err(|e| anyhow!("BUG: ${ENV_ROOT_PACKAGE_SETTINGS} is unreadable: {e}"))?;

        let arg0 = env::args().nth(1);
        let args = env::args().skip(1).collect();
        let vars = env::vars().collect();
        return rustc_wrapper::main(green, arg0, args, vars).await;
    }

    if args.next().as_deref() != Some("green") {
        supergreen::help();
        exit(1)
    }

    let arg2 = args.next();

    let mut cmd = Command::new(cargo());
    cmd.kill_on_drop(true);
    cmd.env("RUSTC_WRAPPER", arg0);
    if let Some(ref arg2) = arg2 {
        // https://rust-lang.github.io/rustup/overrides.html#toolchain-override-shorthand
        if let Some(toolchain) = arg2.strip_prefix('+') {
            let var = "RUSTUP_TOOLCHAIN";
            if let Some(val) = env::var_os(var) {
                if val != toolchain {
                    println!("Overriding {var}={val:?} to {toolchain:?} for `{PKG} +toolchain`");
                }
            }
            // Special handling: call was `cargo green +toolchain ..` (probably from `alias cargo='cargo green'`).
            // Normally, calls look like `cargo +toolchain green ..` but let's simplify alias creation!
            env::set_var(var, toolchain); // Informs `rustc -vV` when deciding on base-image
        } else {
            cmd.arg(arg2);
        }
    }

    let green = cargo_green::main(&mut cmd).await?;
    cmd.env(ENV_ROOT_PACKAGE_SETTINGS, serde_json::to_string(&green)?);

    if arg2.as_deref() == Some("supergreen") {
        return supergreen::main(green, args.next().as_deref(), args.collect()).await;
    }
    cmd.args(args);

    //TODO: https://github.com/messense/cargo-options/blob/086d7470cae34b0e694a62237e258fbd35384e93/examples/cargo-mimic.rs
    // maybe https://lib.rs/crates/clap-cargo

    let command = env::args().nth(2);
    if command.as_deref() == Some("fetch") {
        // Runs actual `cargo fetch`
        if !cmd.status().await?.success() {
            exit(1)
        }
        return cargo_green::fetch(green).await;
    }

    //TODO: also for cfetch

    if !cmd.status().await?.success() {
        exit(1)
    }
    Ok(())
}
