use std::{
    fs::{File, OpenOptions},
    io::Write,
};

use anyhow::{Result, anyhow};
use chrono::Utc;
use env_logger::{Builder, Env, Target};
use log::Level;

use crate::green::Green;

impl Green {
    pub(crate) fn setup_logging(&self, target: &str) -> Result<()> {
        let Some(log_path) = self.env(CARGOGREEN_LOG_PATH!()) else { return Ok(()) };
        let Some(true) = self.env(CARGOGREEN_LOG!()).map(|x| !x.is_empty()) else { return Ok(()) };

        fn log_file(log_path: &str) -> Result<File> {
            let errf = |e| anyhow!("Failed opening (WA) log file {log_path}: {e}");
            OpenOptions::new().create(true).append(true).open(log_path).map_err(errf)
        }

        Builder::from_env(
            Env::default()
                .filter_or(CARGOGREEN_LOG!(), "debug")
                .write_style(CARGOGREEN_LOG_STYLE!()),
        )
        .format({
            let target = target.to_owned();
            move |buf, record| {
                let now = Utc::now().format("%y/%m/%d %H:%M:%S%.3f");
                let lvl = log_level_for_logging(record.level());
                writeln!(buf, "{lvl} {now} {target} {}", record.args())
            }
        })
        .target(Target::Pipe(Box::new(log_file(log_path)?)))
        // A process only ever wraps one rustc call, so the first installation is the right one.
        // Tests however share a process: don't let the losers of that race panic.
        .try_init()?;
        Ok(())
    }
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
