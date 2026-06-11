use std::{collections::BTreeMap, env};

use anyhow::{anyhow, bail, Result};
use camino::Utf8PathBuf;
use tokio::process::Command;

use crate::{ext::CommandExt, green::Green};

#[macro_use]
mod build_script;
mod envs;
mod mds;
mod rustc;

pub(crate) use build_script::*;
pub(crate) use envs::*;
pub(crate) use rustc::*;

// NOTE: this RUSTC_WRAPPER program only ever gets called by `cargo`, so we save
//       ourselves some trouble and assume std::path::{Path, PathBuf} are UTF-8.

#[macro_export]
macro_rules! ENV {
    () => {
        "CARGOGREEN"
    };
}

pub(crate) async fn rustc(
    green: Green,
    arg0: Option<String>,
    args: Vec<String>,
    vars: BTreeMap<String, String>,
) -> Result<()> {
    let argz = args.iter().take(3).map(AsRef::as_ref).collect::<Vec<_>>();

    let argv = |times| args.clone().into_iter().skip(times).collect();
    let is_rustc = |bin: &str| bin.ends_with("rustc");

    match &argz[..] {
        [bin, "--crate-name", ..] if is_rustc(bin) => {
            wrap_rustc(green, argv(1), call_rustc(bin, argv(1))).await
        }
        [driver, bin, "-" | "--crate-name", ..] if is_rustc(bin) => {
            // TODO: wrap driver? + rustc
            // driver: e.g. $RUSTUP_HOME/toolchains/stable-x86_64-unknown-linux-gnu/bin/clippy-driver
            // cf. https://github.com/rust-lang/rust-clippy/tree/da27c979e29e78362b7a2a91ebcf605cb01da94c#using-clippy-driver
            call_rustc(driver, argv(2)).await
        }
        [_driver, bin, ..] if is_rustc(bin) => call_rustc(bin, argv(2)).await,
        [bin, ..] if is_rustc(bin) => call_rustc(bin, argv(1)).await,
        _ => panic!("BUG: RUSTC_WRAPPER={arg0:?}'s input unexpected:\n\targz = {argz:?}\n\targs = {args:?}\n\tenvs = {vars:?}\n"),
    }
}

#[test]
fn passthrough_getting_rust_target_specific_information() {
    #[rustfmt::skip]
    let first_few_args = &[
        "$PWD/rustcbuildx/rustcbuildx",
        "$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc",
        "-",
        "--crate-name", "___",
        "--print=file-names",
        "--crate-type", "bin",
        "--crate-type", "rlib",
        "--crate-type", "dylib",
        "--crate-type", "cdylib",
        "--crate-type", "staticlib",
        "--crate-type", "proc-macro",
        "--print=sysroot",
        "--print=split-debuginfo",
        "--print=crate-name",
        "--print=cfg",
    ]
    .into_iter()
    .take(4)
    .map(ToOwned::to_owned)
    .collect::<Vec<String>>();

    let first_few_args =
        first_few_args.iter().skip(1).take(3).map(String::as_str).collect::<Vec<_>>();
    assert_eq!(
        match &first_few_args[..] {
            [_rustc, "-", ..] | [_rustc, _ /*driver*/, "-", ..] => 1,
            [_rustc, "--crate-name", _crate_name, ..] => 2,
            _ => 3,
        },
        1
    );
}

/// NOTE: not running inside Docker: local install SHOULD match Docker image setup
/// Meaning: it's up to the user to craft their desired $CARGOGREEN_BASE_IMAGE
async fn call_rustc(rustc: &str, args: Vec<String>) -> Result<()> {
    let mut cmd = Command::new(rustc);
    let cmd = cmd.kill_on_drop(true).args(args);
    let status = cmd
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn {}: {e}", cmd.show()))?
        .wait()
        .await
        .map_err(|e| anyhow!("Failed to wait {}: {e}", cmd.show()))?;
    if !status.success() {
        bail!("Failed in call_rustc")
    }
    Ok(())
}

pub(crate) fn call_config() -> (Option<String>, String, String, Utf8PathBuf) {
    (
        env::var("CARGO_CRATE_NAME").ok(), // Unset when executing buildrs (always set when building)
        env::var("CARGO_PKG_NAME").expect("$CARGO_PKG_NAME"),
        env::var("CARGO_PKG_VERSION").expect("$CARGO_PKG_VERSION"),
        env::var("CARGO_MANIFEST_DIR").expect("$CARGO_MANIFEST_DIR").into(),
    )
}
