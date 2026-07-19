use std::{
    env,
    fs::{File, OpenOptions},
    io::Write,
};

use anyhow::{Result, anyhow};
use chrono::Utc;
use env_logger::{Builder, Env, Target};
use log::Level;

pub(crate) fn setup(target: &str) {
    let Some(log_file) = maybe_log() else { return };

    Builder::from_env(
        Env::default().filter_or(CARGOGREEN_LOG!(), "debug").write_style(CARGOGREEN_LOG_STYLE!()),
    )
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
        let log_path = env::var(CARGOGREEN_LOG_PATH!()).expect("set log path earlier");
        let errf = |e| anyhow!("Failed opening (WA) log file {log_path}: {e}");
        OpenOptions::new().create(true).append(true).open(&log_path).map_err(errf)
    }

    env::var(CARGOGREEN_LOG!()).ok().map(|x| !x.is_empty()).unwrap_or_default().then_some(log_file)
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
