use std::{env, fs};

use anyhow::{anyhow, Result};
use camino::Utf8PathBuf;

pub(crate) fn tmp() -> Utf8PathBuf {
    env::temp_dir().try_into().expect("$TMPDIR is not utf-8")
}

pub(crate) fn pwd() -> Utf8PathBuf {
    env::current_dir()
        .expect("$PWD does not exist or is otherwise unreadable")
        .try_into()
        .expect("$PWD is not utf-8")
}

pub(crate) fn cargo_home() -> Result<Utf8PathBuf> {
    home::cargo_home()
        .map_err(|e| anyhow!("Bad $CARGO_HOME or something: {e}"))?
        .try_into()
        .map_err(|e| anyhow!("Corrupted $CARGO_HOME path: {e}"))
}

pub(crate) fn create_current_target_dir(command: Option<&str>) -> Result<String> {
    //FIXME: check precedence
    let target_dir = if let Ok(target_dir) = env::var("CARGO_TARGET_DIR") {
        target_dir
    } else if let Some(target_dir) = {
        let mut args = pico_args::Arguments::from_env();
        args.opt_value_from_str("--target-dir")?
    } {
        target_dir
    } else if command == Some("install") {
        tmp().join(hashed_args()).to_string() //FIXME also add used envs, at least some such as RUSTFLAGS
    } else {
        pwd().join("target").to_string()
    };

    fs::create_dir_all(&target_dir)?;

    let target_dir = camino::Utf8PathBuf::from(target_dir).canonicalize_utf8().unwrap();
    Ok(format!("{target_dir}/")) // Trailing slash required when replacing strings
}

pub(crate) fn hash(string: &str) -> String {
    let h = format!("{:#x}", crc32fast::hash(string.as_bytes())); //~ 0x..
    h["0x".len()..].to_owned()
}

pub(crate) fn hashed_args() -> String {
    hash(&env::args().collect::<Vec<_>>().join(" "))
}
