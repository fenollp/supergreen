use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    path::PathBuf,
    process::exit,
};

use anyhow::{anyhow, bail, Result};
use supergreen::just_help;
use tokio::process::Command;

#[macro_use]
mod add;
#[macro_use]
mod experiments;
#[macro_use]
mod base_image;
mod build;
#[macro_use]
mod builder;
#[macro_use]
mod cache;
mod buildkitd;
#[macro_use]
mod buildrs_wrapper;
mod cargo_green;
mod checkouts;
mod containerfile;
mod cratesio;
mod du;
mod ext;
mod relative;
mod target_dir;
#[macro_use]
mod r#final;
#[macro_use]
mod green;
mod image_uri;
mod lockfile;
#[macro_use]
mod logging;
mod md;
mod network;
mod rechrome;
#[macro_use]
mod runner;
mod rustc_arguments;
#[macro_use]
mod rustc_wrapper;
mod rustup;
mod stage;
mod supergreen;

const PKG: &str = env!("CARGO_PKG_NAME");
const REPO: &str = env!("CARGO_PKG_REPOSITORY");
const VSN: &str = env!("CARGO_PKG_VERSION");

// TODO: make this actually show up in `cargo --list`
cargo_subcommand_metadata::description! {
    "Sandbox & cache cargo builds and execute jobs remotely"
}

fn cargo() -> OsString {
    env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

// Internal env used to pass config from cargo plugin to rustc wrapper
const ENV_ROOT_PACKAGE_SETTINGS: &str = "CARGOGREEN_ROOT_PACKAGE_SETTINGS_";

// Subcommands that need our wrapping
#[rustfmt::skip]
const HANDLED: &[&str] = &[
    "supergreen",
    "b", "bench", "build",
    "c", "check",
    "clippy",
    "d", "doc",
    "fetch",
    "fix",
    "install",
    "package",
    "publish",
    "r", "run",
    "rustc",
    "rustdoc",
    "t", "test",
];
// ...ones we know that don't:
// (naked) add clean config fmt generate-lockfile help info init locate-project
//         login logout metadata new owner pkgid read-manifest remove report rm
//         search tree uninstall update vendor verify-project version yank

#[tokio::main]
async fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    let mut args = env::args();

    let arg0 = args.next().expect("$0 has to be set");
    if PathBuf::from(&arg0).file_name() != Some(OsStr::new(PKG)) {
        bail!("This binary should be named `{PKG}`")
    }

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
            return buildrs_wrapper::exec_buildrs(green, exe.into()).await;
        }

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

    let mut cmd = Command::new(cargo());
    cmd.kill_on_drop(true);
    if let Some(ref arg2) = arg2 {
        cmd.arg(arg2);
    }
    cmd.args(args);

    // TODO: handle `-Z bla` (works: `-Zbla`)
    let subcommand_start = || env::args().skip(2).skip_while(|arg| arg.starts_with('-'));
    let command = subcommand_start().next();

    if !command.as_deref().map(|c| HANDLED.contains(&c)).unwrap_or(false) {
        if !cmd.status().await?.success() {
            exit(1)
        }
        return Ok(());
    }
    cmd.env("RUSTC_WRAPPER", arg0);

    // Shortcut here just for `cargo green supergreen --help` to avoid some calculations
    if command.as_deref() == Some("supergreen") {
        let first_arg = subcommand_start().find(|arg| arg != "supergreen");
        if just_help(first_arg.as_deref()) {
            supergreen::help();
            return Ok(());
        }
    }

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

    // Goal: produce only fully-locked Dockerfiles/TOMLs

    let green = cargo_green::main().await?;
    cmd.env(ENV_ROOT_PACKAGE_SETTINGS, serde_json::to_string(&green)?);

    if command.as_deref() == Some("supergreen") {
        let mut args = subcommand_start();
        let first_arg = args.find(|arg| arg != "supergreen");
        return supergreen::main(green, first_arg.as_deref(), args.collect()).await;
    }

    if command.as_deref() == Some("fetch") {
        // Runs actual `cargo fetch`
        if !cmd.status().await?.success() {
            exit(1)
        }
        return green.prebuild(true).await;
    }
    green.prebuild(false).await?;

    //FIXME: check precedence
    let target_dir = if let Ok(target_dir) = env::var("CARGO_TARGET_DIR") {
        target_dir
    } else if let Some(target_dir) = {
        let mut args = pico_args::Arguments::from_env();
        args.opt_value_from_str("--target-dir")?
    } {
        target_dir
    } else if command.as_deref() == Some("install") {
        tmp().join(hashed_args()).to_string() //FIXME also add used envs, at least some such as RUSTFLAGS
    } else {
        pwd().join("target").to_string()
    };
    fs::create_dir_all(&target_dir)?;
    let target_dir = camino::Utf8PathBuf::from(target_dir).canonicalize_utf8().unwrap();
    let target_dir = format!("{target_dir}/"); // Trailing slash required when replacing strings
    cmd.env("CARGO_TARGET_DIR", &target_dir);
    env::set_var("CARGO_TARGET_DIR", target_dir);

    if !cmd.status().await?.success() {
        exit(1)
    }
    Ok(())
}

fn tmp() -> camino::Utf8PathBuf {
    env::temp_dir().try_into().unwrap()
}

fn pwd() -> camino::Utf8PathBuf {
    env::current_dir()
        .expect("$PWD does not exist or is otherwise unreadable")
        .try_into()
        .expect("$PWD is not utf-8")
}

fn hash(string: &str) -> String {
    let h = format!("{:#x}", crc32fast::hash(string.as_bytes())); //~ 0x..
    h["0x".len()..].to_owned()
}

fn hashed_args() -> String {
    hash(&env::args().collect::<Vec<_>>().join(" "))
}
