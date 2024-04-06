use std::{
    collections::BTreeMap,
    env,
    process::{ExitCode, Stdio},
};

use anyhow::{Context, Result};
use tokio::process::Command;

use crate::envs::{base_image, internal, log_path, runner, syntax};

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
        ("RUSTCBUILDX", internal::this()),
        ("RUSTCBUILDX_BASE_IMAGE", Some(base_image().await.to_owned())),
        ("RUSTCBUILDX_LOG", internal::log()),
        ("RUSTCBUILDX_LOG_PATH", Some(log_path().to_owned())),
        ("RUSTCBUILDX_LOG_STYLE", internal::log_style()),
        ("RUSTCBUILDX_RUNNER", Some(runner().to_owned())),
        ("RUSTCBUILDX_SYNTAX", Some(syntax().await.to_owned())),
    ]
    .into_iter()
    .collect();

    fn show(var: &str, o: &Option<String>) {
        println!("{var}={val}", val = o.as_deref().unwrap_or_default());
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
    let command = runner();
    let mut failure = ExitCode::SUCCESS;
    for (user_input, img) in
        [(internal::syntax(), syntax().await), (internal::base_image(), base_image().await)]
    {
        let img = img.trim_start_matches("docker-image://");
        let img = if img.contains('@')
            && (user_input.is_none() || user_input.map(|x| !x.contains('@')).unwrap_or_default())
        {
            // Don't pull a locked image unless that's what's asked
            // Otherwise, pull unlocked

            // The only possible cases (user_input sets img)
            // none + @ = trim
            // none + _ = _
            // s @  + @ = _
            // s !  + @ = trim
            img.trim_end_matches(|c| c != '@').trim_end_matches('@')
        } else {
            img
        };
        println!("Pulling {img}...");

        let o = Command::new(command)
            .kill_on_drop(true)
            .arg("pull")
            .arg(img)
            .stdin(Stdio::null())
            .spawn()
            .with_context(|| format!("Failed to start `{command} pull {img}`"))?
            .wait()
            .await
            .with_context(|| format!("Failed to call `{command} pull {img}`"))?;
        if !o.success() {
            failure = exit_code(o.code());
        }
        println!();
    }
    Ok(failure)
}

#[inline]
pub(crate) fn exit_code(code: Option<i32>) -> ExitCode {
    // TODO: https://doc.rust-lang.org/std/os/unix/process/trait.ExitStatusExt.html
    (code.unwrap_or(-1) as u8).into()
}
