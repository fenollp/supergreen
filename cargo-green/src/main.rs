use std::{env, ffi::OsStr, fs, path::PathBuf};

use anyhow::{Result, anyhow, bail};
use tokio::process::Command;

use crate::{
    all_our_envs::{CARGOGREEN_LOG_PATH, CARGOGREEN_PLUGINSETTINGS, RUSTC_WRAPPER},
    dirs::{create_current_target_dir, hashed_args, tmp},
};

#[macro_use]
mod all_our_envs;

mod add;
mod base_image;
mod build;
mod builder;
mod buildkitd;
mod cache;
mod cargo_arguments;
mod cargo_green;
mod checkouts;
mod containerfile;
mod cratesio;
mod dirs;
mod du;
mod experiments;
mod ext;
mod r#final;
mod green;
mod image_uri;
mod lockfile;
mod logging;
mod md;
mod network;
mod rechrome;
mod relative;
mod retrier;
mod runner;
mod rustc_arguments;
mod rustup;
mod stage;
mod supergreen;
mod target_dir;
mod wrap;

const PKG: &str = env!("CARGO_PKG_NAME");
const REPO: &str = env!("CARGO_PKG_REPOSITORY");
const VSN: &str = env!("CARGO_PKG_VERSION");

// TODO: make this actually show up in `cargo --list`
cargo_subcommand_metadata::description! {
    "Sandbox & cache cargo builds and execute jobs remotely"
}

const EEXIT: &str = "";

fn main() -> Result<()> {
    if let Err(e) = actual_main() {
        if format!("{e}") == EEXIT {
            std::process::exit(1)
        }
        return Err(e);
    }
    Ok(())
}

fn actual_main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    let mut args = env::args();

    let arg0 = args.next().expect("$0 has to be set");
    if PathBuf::from(&arg0).file_name() != Some(OsStr::new(PKG)) {
        bail!("This binary should be named `{PKG}`")
    }

    if let Ok(wrapper) = env::var(RUSTC_WRAPPER!()) {
        // Now running as a subprocess

        if PathBuf::from(&wrapper).file_name() != Some(OsStr::new(PKG)) {
            bail!("A {RUSTC_WRAPPER} other than `{PKG}` is already set: {wrapper}")
        }

        let green = env::var(CARGOGREEN_PLUGINSETTINGS!())
            .map_err(|e| anyhow!("BUG: {CARGOGREEN_PLUGINSETTINGS} is unset: {e}"))?;
        let green = serde_json::from_str(&green)
            .map_err(|e| anyhow!("BUG: {CARGOGREEN_PLUGINSETTINGS} is unreadable: {e}"))?;

        if let Some(weird) = env::var_os(CARGOGREEN!()) {
            bail!("It's turtles all the way down! ({weird:?})")
        }
        // SAFETY: environment access only happens in single-threaded code.
        unsafe { env::set_var(CARGOGREEN!(), "1") };

        return block_on(async {
            // Dance to wrap build script execution: we patched the build.rs to call us back through here.
            if let Ok(exe) = env::var(CARGOGREEN_EXECUTEBUILDSCRIPT!()) {
                return wrap::exec_build_script(green, exe.into()).await;
            }

            let arg0 = env::args().nth(1);
            let args = env::args().skip(1).collect();
            let vars = env::vars().collect();
            wrap::rustc(green, arg0, args, vars).await
        });
    }

    block_on(really_actual_main(arg0, args))
}

fn block_on(f: impl Future<Output = Result<()>>) -> Result<()> {
    tokio::runtime::Builder::new_multi_thread().enable_all().name(PKG).build().unwrap().block_on(f)
}

async fn really_actual_main(arg0: String, mut args: env::Args) -> Result<()> {
    if args.next().as_deref() != Some("green") {
        supergreen::help();
        bail!(EEXIT)
    }

    let Some((cargo, toolchain)) = env::var_os(CARGO!()).zip(env::var(RUSTUP_TOOLCHAIN!()).ok())
    else {
        eprintln!("This cargo plugin must be run like `cargo green ...`");
        bail!(EEXIT)
    };

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

    let mut cmd = Command::new(cargo);
    cmd.kill_on_drop(true);
    if let Some(ref arg2) = arg2 {
        cmd.arg(arg2);
    }
    cmd.args(args);

    let command = cargo_arguments::subcommand(env::args().skip(2));

    if let Some(ref command) = command {
        let close = matches!(strsim::damerau_levenshtein("green", command), 0..=2)
            || matches!(strsim::damerau_levenshtein("supergreen", command), 1..=3);
        if close {
            bail!("Did you mean 'supergreen' by typing {command:?}?")
        }
    }

    #[rustfmt::skip]
    let handled = command.as_deref().is_some_and(|arg| {
        // Subcommands that needn't our wrapping:
        // (naked) add clean config fix fmt generate-lockfile help info init
        // locate-project login logout metadata new owner pkgid read-manifest
        // remove report rm search tree uninstall update vendor verify-project
        // version yank
            matches!(
                arg,
                "supergreen" | "b" | "bench" | "build" | "c" | "check" |
                "clippy" | "d" | "doc" | "fetch" | "install" | "package" |
                "publish" | "r" | "run" | "rustc" | "rustdoc" | "t" | "test"
            )
    });
    let is_install = command.as_deref() == Some("install");

    if !handled {
        if !cmd.status().await?.success() {
            bail!(EEXIT)
        }
        return Ok(());
    }
    cmd.env(RUSTC_WRAPPER!(), arg0);

    // TODO: TUI above cargo output (? https://docs.rs/prodash )

    if let Ok(log) = env::var(CARGOGREEN_LOG!()) {
        cmd.env(CARGOGREEN_LOG!(), log);
        let path = env::var(CARGOGREEN_LOG_PATH!())
            .unwrap_or_else(|_| tmp().join(format!("{PKG}-{}.log", hashed_args())).to_string());
        let path = camino::absolute_utf8(path)
            .map_err(|e| anyhow!("Failed canonicalizing {CARGOGREEN_LOG_PATH}: {e}"))?;
        cmd.env(CARGOGREEN_LOG_PATH!(), &path);
        let _ = fs::OpenOptions::new().create(true).truncate(false).append(true).open(path);
    }

    assert!(env::var_os(CARGOGREEN!()).is_none());
    assert!(env::var_os(CARGOGREEN_PLUGINSETTINGS!()).is_none());

    // Shortcut here just for `cargo green supergreen --help` to avoid some calculations
    if supergreen::just_help() {
        supergreen::help();
        return Ok(());
    }

    let green = cargo_green::main(&toolchain, is_install).await?;
    cmd.env(CARGOGREEN_PLUGINSETTINGS!(), serde_json::to_string(&green)?);

    match command.as_deref() {
        Some("supergreen") => supergreen::main(green).await,
        Some("fetch") => {
            // Runs actual `cargo fetch`
            if !cmd.status().await?.success() {
                bail!(EEXIT)
            }
            green.prebuild(true, is_install).await
        }
        _ => {
            green.prebuild(false, is_install).await?;
            cmd.env(CARGO_TARGET_DIR!(), create_current_target_dir(is_install)?);
            if !cmd.status().await?.success() {
                bail!(EEXIT)
            }
            Ok(())
        }
    }
}
