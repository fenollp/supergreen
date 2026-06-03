use std::{env, ffi::OsStr, fs, path::PathBuf};

use anyhow::{anyhow, bail, Result};
use tokio::process::Command;

use crate::dirs::{create_current_target_dir, hashed_args, tmp};

#[macro_use]
mod add;
#[macro_use]
mod base_image;
#[macro_use]
mod builder;
#[macro_use]
mod cache;
#[macro_use]
mod experiments;
#[macro_use]
mod r#final;
#[macro_use]
mod green;
#[macro_use]
mod logging;
#[macro_use]
mod runner;
#[macro_use]
mod wrap;

mod build;
mod buildkitd;
mod cargo_green;
mod checkouts;
mod containerfile;
mod cratesio;
mod dirs;
mod du;
mod ext;
mod image_uri;
mod lockfile;
mod md;
mod network;
mod rechrome;
mod relative;
mod rustc_arguments;
mod rustup;
mod stage;
mod supergreen;
mod target_dir;

const PKG: &str = env!("CARGO_PKG_NAME");
const REPO: &str = env!("CARGO_PKG_REPOSITORY");
const VSN: &str = env!("CARGO_PKG_VERSION");

// TODO: make this actually show up in `cargo --list`
cargo_subcommand_metadata::description! {
    "Sandbox & cache cargo builds and execute jobs remotely"
}

const EEXIT: &str = "";

#[tokio::main]
async fn main() -> Result<()> {
    if let Err(e) = actual_main().await {
        if format!("{e}") == EEXIT {
            std::process::exit(1)
        }
        return Err(e);
    }
    Ok(())
}

async fn actual_main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    let mut args = env::args();

    let arg0 = args.next().expect("$0 has to be set");
    if PathBuf::from(&arg0).file_name() != Some(OsStr::new(PKG)) {
        bail!("This binary should be named `{PKG}`")
    }

    // Internal env used to pass config from cargo plugin to rustc wrapper
    const ENV_ROOT_PACKAGE_SETTINGS: &str = "CARGOGREEN_ROOT_PACKAGE_SETTINGS_";

    if let Ok(wrapper) = env::var("RUSTC_WRAPPER") {
        // Now running as a subprocess

        if PathBuf::from(&wrapper).file_name() != Some(OsStr::new(PKG)) {
            bail!("A $RUSTC_WRAPPER other than `{PKG}` is already set: {wrapper}")
        }

        let green = env::var(ENV_ROOT_PACKAGE_SETTINGS)
            .map_err(|_| anyhow!("BUG: ${ENV_ROOT_PACKAGE_SETTINGS} is unset"))?;
        let green = serde_json::from_str(&green)
            .map_err(|e| anyhow!("BUG: ${ENV_ROOT_PACKAGE_SETTINGS} is unreadable: {e}"))?;

        // Dance to wrap build script execution: we patched the build.rs to call us back through here.
        if let Ok(exe) = env::var(ENV_EXECUTE_BUILDRS!()) {
            return wrap::exec_build_script(green, exe.into()).await;
        }

        let arg0 = env::args().nth(1);
        let args = env::args().skip(1).collect();
        let vars = env::vars().collect();
        return wrap::rustc(green, arg0, args, vars).await;
    }

    if args.next().as_deref() != Some("green") {
        supergreen::help();
        bail!(EEXIT)
    }

    let arg2 = args.next();

    // https://rust-lang.github.io/rustup/overrides.html#toolchain-override-shorthand
    if let Some(toolchain) = arg2.as_ref().and_then(|arg2| arg2.strip_prefix('+')) {
        // Special handling: call was `cargo green +toolchain ..` (probably from `alias cargo='cargo green'`).
        //  Let's flip this back into `cargo +toolchain green ..`
        let mut cmd = Command::new("cargo");
        cmd.arg(format!("+{toolchain}"));
        cmd.arg("green");
        cmd.args(args);
        cmd.kill_on_drop(true);
        return cmd.status().await.map(|_| ()).map_err(Into::into);
    }

    let mut cmd = Command::new(env::var_os("CARGO").expect("$CARGO"));
    cmd.kill_on_drop(true);
    if let Some(ref arg2) = arg2 {
        cmd.arg(arg2);
    }
    cmd.args(args);

    // TODO: handle `-Z bla` (works: `-Zbla`)
    let command = env::args().skip(2).find(|arg| !arg.starts_with('-'));

    #[rustfmt::skip]
    let handled = command.as_deref().is_some_and(|arg| {
        // Subcommands that needn't our wrapping:
        // (naked) add clean config fmt generate-lockfile help info init locate-project
        //         login logout metadata new owner pkgid read-manifest remove report rm
        //         search tree uninstall update vendor verify-project version yank
            matches!(
                arg,
                "supergreen" | "b" | "bench" | "build" | "c" | "check" | "clippy" |
                "d" | "doc" | "fetch" | "fix" | "install" | "package" | "publish" |
                "r" | "run" | "rustc" | "rustdoc" | "t" | "test"
            )
    });

    if !handled {
        if !cmd.status().await?.success() {
            bail!(EEXIT)
        }
        return Ok(());
    }
    cmd.env("RUSTC_WRAPPER", arg0);

    // TODO: TUI above cargo output (? https://docs.rs/prodash )

    if let Ok(log) = env::var(ENV_LOG!()) {
        cmd.env(ENV_LOG!(), log);
        let var = ENV_LOG_PATH!();
        let path = env::var(var)
            .unwrap_or_else(|_| tmp().join(format!("{PKG}-{}.log", hashed_args())).to_string());
        let path = camino::absolute_utf8(path)
            .map_err(|e| anyhow!("Failed canonicalizing ${var}: {e}"))?;
        env::set_var(var, &path);
        cmd.env(var, &path);
        let _ = fs::OpenOptions::new().create(true).truncate(false).append(true).open(path);
    }

    assert!(env::var_os(ENV!()).is_none());
    assert!(env::var_os(ENV_ROOT_PACKAGE_SETTINGS).is_none());

    // Shortcut here just for `cargo green supergreen --help` to avoid some calculations
    if supergreen::just_help() {
        supergreen::help();
        return Ok(());
    }

    let green = cargo_green::main().await?;
    cmd.env(ENV_ROOT_PACKAGE_SETTINGS, serde_json::to_string(&green)?);

    if command.as_deref() == Some("supergreen") {
        return supergreen::main(green).await;
    }

    if command.as_deref() == Some("fetch") {
        // Runs actual `cargo fetch`
        if !cmd.status().await?.success() {
            bail!(EEXIT)
        }
        return green.prebuild(true).await;
    }
    green.prebuild(false).await?;

    let target_dir = create_current_target_dir(command.as_deref())?;
    cmd.env("CARGO_TARGET_DIR", &target_dir);
    env::set_var("CARGO_TARGET_DIR", target_dir);

    if !cmd.status().await?.success() {
        bail!(EEXIT)
    }
    Ok(())
}
