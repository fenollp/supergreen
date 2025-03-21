use std::io::Write;

use chrono::Utc;
use env_logger::{Builder, Env, Target};
use log::Level;

use crate::envs::maybe_log;

pub(crate) fn setup(target: String, log_env: &str, log_style_env: &str) {
    let Some(log_file) = maybe_log() else { return };

    Builder::from_env(Env::default().filter_or(log_env, "debug").write_style(log_style_env))
        .format(move |buf, record| {
            let now = Utc::now().format("%y-%m-%dT%H:%M:%S%.3f");
            let lvl = match record.level() {
                Level::Error => 'E',
                Level::Warn => 'W',
                Level::Info => 'I',
                Level::Debug => 'D',
                Level::Trace => 'T',
            };
            writeln!(buf, "[{now} {lvl} {target}] {}", record.args())
        })
        .target(Target::Pipe(Box::new(log_file().expect("Installing logfile"))))
        .init();
}
