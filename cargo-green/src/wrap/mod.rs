use std::{
    collections::BTreeMap,
    env, fmt,
    ops::{Deref, DerefMut},
};

use anyhow::{Result, anyhow, bail};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::{
    all_our_envs::{CARGO_MANIFEST_DIR, CARGO_PKG_NAME, CARGO_PKG_VERSION, RUSTC_WRAPPER},
    green::Green,
    redact::redact,
};

mod build_script;
mod envs;
mod mds;
mod rustc;

pub(crate) use build_script::*;
pub(crate) use envs::*;
pub(crate) use rustc::*;

// NOTE: this RUSTC_WRAPPER program only ever gets called by `cargo`, so we save
//       ourselves some trouble and assume std::path::{Path, PathBuf} are UTF-8.

/// A snapshot of the process environment, read once in `main`.
///
/// Its `Debug` output redacts values, see [`crate::redact`].
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Vars(BTreeMap<String, String>);

impl fmt::Debug for Vars {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter().map(|(var, val)| (var, redact(var, val)))).finish()
    }
}

impl Deref for Vars {
    type Target = BTreeMap<String, String>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Vars {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl FromIterator<(String, String)> for Vars {
    fn from_iter<I: IntoIterator<Item = (String, String)>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl<const N: usize> From<[(String, String); N]> for Vars {
    fn from(kvs: [(String, String); N]) -> Self {
        Self(kvs.into())
    }
}

impl<'a> IntoIterator for &'a Vars {
    type Item = (&'a String, &'a String);
    type IntoIter = std::collections::btree_map::Iter<'a, String, String>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[test]
fn vars_debug_redacts() {
    let env: Vars = [("GH_TOKEN", "0123456789"), ("CARGO_PKG_NAME", "green")]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    assert_eq!(format!("{env:?}"), r#"{"CARGO_PKG_NAME": "green", "GH_TOKEN": "01234***"}"#);
}

pub(crate) async fn rustc(
    green: Green,
    arg0: String,
    args: env::Args,
    pwd: Utf8PathBuf,
) -> Result<()> {
    let args: Vec<String> = args.collect();
    let argz: Vec<&str> = args.iter().take(3).map(AsRef::as_ref).collect();

    let argv = |times| args.iter().skip(times).cloned().collect();
    let is_rustc = |bin: &str| bin.ends_with("rustc");

    match argz.as_slice() {
        [rustc, "--crate-name", ..] if is_rustc(rustc) => {
            wrap_rustc(green, argv(1), &pwd, call_rustc(rustc, argv(1))).await
        }
        [driver, rustc, "-" | "--crate-name", ..] if is_rustc(rustc) => {
            // TODO: wrap driver? + rustc
            // driver: e.g. $RUSTUP_HOME/toolchains/stable-x86_64-unknown-linux-gnu/bin/clippy-driver
            // cf. https://github.com/rust-lang/rust-clippy/tree/da27c979e29e78362b7a2a91ebcf605cb01da94c#using-clippy-driver
            call_rustc(driver, argv(2)).await
        }
        [_driver, rustc, ..] if is_rustc(rustc) => call_rustc(rustc, argv(2)).await,
        [rustc, ..] if is_rustc(rustc) => call_rustc(rustc, argv(1)).await,
        _ => bail!(
            "BUG: {RUSTC_WRAPPER}={arg0}'s input unexpected:\n\targs = {args:?}\n\tenvs = {vars:?}\n",
            vars = green.env,
        ),
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
    let cmd = cmd.kill_on_drop(true).args(&args);
    let status = cmd
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn {rustc} {args:?}: {e}"))?
        .wait()
        .await
        .map_err(|e| anyhow!("Failed to wait on {rustc} {args:?}: {e}"))?;
    if !status.success() {
        bail!("Failed in {rustc} {args:?}")
    }
    Ok(())
}

impl Green {
    pub(crate) fn call_config(&self) -> (Option<String>, String, String, Utf8PathBuf) {
        let env = |name| self.env(name).map(ToOwned::to_owned);
        (
            env(CARGO_CRATE_NAME!()), // Unset when executing buildrs (always set when building)
            env(CARGO_PKG_NAME!()).expect(CARGO_PKG_NAME),
            env(CARGO_PKG_VERSION!()).expect(CARGO_PKG_VERSION),
            env(CARGO_MANIFEST_DIR!()).expect(CARGO_MANIFEST_DIR).into(),
        )
    }
}
