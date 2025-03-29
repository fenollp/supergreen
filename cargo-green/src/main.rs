use std::{env, ffi::OsStr, path::PathBuf, process::exit};

use anyhow::{bail, Result};
use cargo_toml::Manifest;
use envs::internal;
use serde::Deserialize;
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
                if val != toolchain {
                    println!("Overriding {var}={val:?} to {toolchain:?} for `{PKG} +toolchain`");
                }
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

    // // cargo green: intercept buildrs runs with
    // // RUSTFLAGS="-C linker=/use/bin/cc"
    // cmd.env(
    //     "RUSTFLAGS",
    //     format!(
    //         // "{} -C linker=/home/pete/wefwefwef/supergreen.git/linker.sh",
    //         "{} -C linker=/linker.sh",
    //         std::env::var("RUSTFLAGS").unwrap_or_default()
    //     ),
    // );
    // // https://doc.rust-lang.org/rustc/codegen-options/index.html#linker

    cmd.env("RUSTC_WRAPPER", arg0);

    cargo_green::main(&mut cmd).await?;

    let syntax = internal::syntax().expect("set in cargo_green::main");

    //TODO: https://github.com/messense/cargo-options/blob/086d7470cae34b0e694a62237e258fbd35384e93/examples/cargo-mimic.rs
    // maybe https://lib.rs/crates/clap-cargo

    if false {
        // let manifest = match GreenCli::try_parse_from(env::args().skip(4)) {
        //     Ok(GreenCli { manifest }) => manifest,
        //     Err(e) => bail!(">>> {e}"),
        // };

        // let manifest = clap_cargo::Manifest::default();
        let manifest_path = std::env::current_dir().expect("$PWD");
        let manifest_path: PathBuf =
                    // std::fs::canonicalize(manifest_path).expect("canon").join("Cargo.toml");
                    std::fs::canonicalize(manifest_path).expect("canon").join("cargo-green/Cargo.toml");
        let manifest = Manifest::from_path(&manifest_path).expect("from");

        // panic!(">>> {:?}", env::vars().collect::<Vec<_>>())

        eprintln!(">>> {manifest_path:?}");
        eprintln!(">>> .package {:?}", manifest.package.clone());
        if let Some(metadata) = manifest.package.as_ref().and_then(|x| x.metadata.as_ref()) {
            eprintln!(">>> {metadata:?}");
            eprintln!(">>> {:?}", toml::to_string(metadata));

            #[derive(Debug, Deserialize)]
            #[allow(dead_code)]
            struct GreenMetadata {
                green: ThisField,
            }
            #[derive(Debug, Deserialize)]
            #[allow(dead_code)]
            struct ThisField {
                this: String,
            }

            let cfg: GreenMetadata =
                toml::from_str(&toml::to_string(metadata).expect("str")).expect("parse");

            eprintln!(">>> {cfg:?}");
        }

        // eprintln!(">>> .workspace {:?}", manifest.workspace.clone());
        // eprintln!(
        //     ">>> .workspace.package {:?}",
        //     manifest.workspace.clone().map(|x| x.package).and_then(|x| x.metadata)
        // );
        panic!(">>> {manifest:?}");
    }

    match env::args().nth(2).as_deref() {
        None => {}
        Some("fetch") => {
            // Runs actual `cargo fetch`
            if !cmd.status().await?.success() {
                exit(1)
            }
            return cargo_green::fetch(&syntax).await;
        }
        Some(_) => {}
    }

    if !cmd.status().await?.success() {
        exit(1)
    }
    Ok(())
}
