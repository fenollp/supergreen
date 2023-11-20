use std::{
    collections::BTreeMap,
    env,
    io::{self, Write},
    process::{Command, ExitCode, Stdio},
};

use anyhow::{Context, Result};

use crate::envs::{base_image, docker_syntax, log_path};

#[inline]
pub(crate) fn help() -> ExitCode {
    let name = env!("CARGO_PKG_NAME");
    println!(
        "{name}@{version}: {description}
    {repository}

Usage:
  {name} env             Show used values
  {name} pull            Pulls images (respects $DOCKER_HOST)
  {name} -h | --help
  {name} -V | --version
",
        description = env!("CARGO_PKG_DESCRIPTION"),
        version = env!("CARGO_PKG_VERSION"),
        repository = env!("CARGO_PKG_REPOSITORY"),
    );
    ExitCode::SUCCESS
}

pub(crate) fn envs(vars: impl Iterator<Item = String>) -> ExitCode {
    //TODO: CI: sort + grep src/envs.rs
    let all: BTreeMap<_, _> = [
        ("RUSTCBUILDX", None),
        ("RUSTCBUILDX_BASE_IMAGE", Some(base_image())),
        ("RUSTCBUILDX_DOCKER_SYNTAX", Some(docker_syntax())),
        ("RUSTCBUILDX_LOG", None),
        ("RUSTCBUILDX_LOG_PATH", Some(log_path())),
        ("RUSTCBUILDX_LOG_STYLE", None),
    ]
    .into_iter()
    .collect();

    fn show(var: &str, o: Option<String>) {
        let val = env::var(var).ok().or(o).map(|x| format!("{x:?}")).unwrap_or_default();
        println!("{var}={val}");
    }

    let mut empty_vars = true;
    for var in vars {
        if let Some(o) = all.get(&var.as_str()) {
            show(&var, o.clone());
            empty_vars = false;
        }
    }
    if empty_vars {
        all.into_iter().for_each(|(var, o)| show(var, o));
    }

    ExitCode::SUCCESS
}

pub(crate) fn pull() -> Result<ExitCode> {
    for img in [docker_syntax(), base_image().trim_start_matches("docker-image://").to_owned()] {
        println!("Pulling {img}...");
        let o = Command::new("docker")
            .arg("pull")
            .arg(&img)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("Failed to call docker pull {img}"))
            .unwrap();
        io::stderr().write_all(&o.stderr).unwrap();
        io::stdout().write_all(&o.stdout).unwrap();
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
