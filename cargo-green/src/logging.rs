use std::{
    env,
    fs::{File, OpenOptions},
    io::Write,
};

use anyhow::{anyhow, Result};
use chrono::Utc;
use env_logger::{Builder, Env, Target};
use log::Level;

pub(crate) const ENV_LOG: &str = "CARGOGREEN_LOG";
pub(crate) const ENV_LOG_PATH: &str = "CARGOGREEN_LOG_PATH";
pub(crate) const ENV_LOG_STYLE: &str = "CARGOGREEN_LOG_STYLE";

pub(crate) fn setup(target: &str) {
    let Some(log_file) = maybe_log() else { return };

    Builder::from_env(Env::default().filter_or(ENV_LOG, "debug").write_style(ENV_LOG_STYLE))
        .format({
            let target = target.to_owned();
            move |buf, record| {
                let now = Utc::now().format("%y/%m/%d %H:%M:%S%.3f");
                let lvl = log_level_for_logging(record.level());
                writeln!(buf, "{lvl} {now} {target} {}", record.args())
            }
        })
        .target(Target::Pipe(Box::new(log_file().expect("Installing logfile"))))
        .init();
}

#[must_use]
pub(crate) fn maybe_log() -> Option<fn() -> Result<File>> {
    fn log_file() -> Result<File> {
        let log_path = env::var(ENV_LOG_PATH).expect("set log path earlier");
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|e| anyhow!("Failed opening (WA) log file {log_path}: {e}"))
    }

    env::var(ENV_LOG).ok().map(|x| !x.is_empty()).unwrap_or_default().then_some(log_file)
}

#[must_use]
fn log_level_for_logging(lvl: Level) -> char {
    match lvl {
        Level::Error => 'E',
        Level::Warn => 'W',
        Level::Info => 'I',
        Level::Debug => 'D',
        Level::Trace => 'T',
    }
}

#[must_use]
pub(crate) fn crate_type_for_logging(crate_type: &str) -> char {
    crate_type.chars().next().unwrap().to_ascii_uppercase()
}

#[test]
fn unique_krate_types() {
    use std::collections::HashSet;

    use super::rustc_arguments::ALL_CRATE_TYPES;

    let all: HashSet<_> = ALL_CRATE_TYPES.iter().map(|ty| crate_type_for_logging(ty)).collect();
    assert_eq!(ALL_CRATE_TYPES.len(), all.len());
    assert!(!all.contains(&'X')); // for build scripts
}
