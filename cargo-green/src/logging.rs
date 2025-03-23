use std::io::Write;

use chrono::Utc;
use env_logger::{Builder, Env, Target};
use log::Level;

use crate::envs::maybe_log;

pub(crate) fn setup(target: &str, log_env: &str, log_style_env: &str) {
    let Some(log_file) = maybe_log() else { return };

    Builder::from_env(Env::default().filter_or(log_env, "debug").write_style(log_style_env))
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
