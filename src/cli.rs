use std::{
    collections::BTreeMap,
    env,
    io::{self, Write},
    process::{ExitCode, Stdio},
};

use anyhow::{Context, Result};
use tokio::process::Command;

use crate::envs::{base_image, log_path, runner, syntax};

#[inline]
pub(crate) fn help() -> ExitCode {
    println!(
        "{name}@{version}: {description}
    {repository}

Usage:
  {name} env             Show used values
  {name} pull            Pulls images (respects $DOCKER_HOST)
  {name} -h | --help
  {name} -V | --version
",
        name = env!("CARGO_PKG_NAME"),
        version = env!("CARGO_PKG_VERSION"),
        repository = env!("CARGO_PKG_REPOSITORY"),
        description = env!("CARGO_PKG_DESCRIPTION"),
    );
    ExitCode::SUCCESS
}

pub(crate) async fn envs(vars: Vec<String>) -> ExitCode {
    let all: BTreeMap<_, _> = [
        ("RUSTCBUILDX", None),
        ("RUSTCBUILDX_BASE_IMAGE", Some(base_image().await)),
        ("RUSTCBUILDX_LOG", None),
        ("RUSTCBUILDX_LOG_PATH", Some(log_path())),
        ("RUSTCBUILDX_LOG_STYLE", None),
        ("RUSTCBUILDX_RUNNER", Some(runner())),
        ("RUSTCBUILDX_SYNTAX", Some(syntax())),
    ]
    .into_iter()
    .collect();

    fn show(var: &str, o: &Option<String>) {
        let val = env::var(var)
            .ok()
            .or_else(|| o.to_owned())
            .map(|x| format!("{x:?}"))
            .unwrap_or_default();
        println!("{var}={val}");
    }

    let mut empty_vars = true;
    for var in vars {
        if let Some(o) = all.get(&var.as_str()) {
            show(&var, o);
            empty_vars = false;
        }
    }
    if empty_vars {
        all.into_iter().for_each(|(var, o)| show(var, &o));
    }

    ExitCode::SUCCESS
}

pub(crate) async fn pull() -> Result<ExitCode> {
    for img in [syntax(), base_image().await.trim_start_matches("docker-image://").to_owned()] {
        println!("Pulling {img}...");
        let o = Command::new(&runner())
            .kill_on_drop(true)
            .arg("pull")
            .arg(&img)
            .stdin(Stdio::null())
            .output()
            .await
            .with_context(|| format!("Failed to call docker pull {img}"))?;
        io::stderr().write_all(&o.stderr)?;
        io::stdout().write_all(&o.stdout)?;
        let o = o.status;
        if !o.success() {
            return Ok(exit_code(o.code()));
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[inline]
pub(crate) fn exit_code(code: Option<i32>) -> ExitCode {
    // TODO: https://doc.rust-lang.org/std/os/unix/process/trait.ExitStatusExt.html
    (code.unwrap_or(-1) as u8).into()
}
