use std::{
    collections::BTreeMap,
    env,
    process::{ExitCode, Stdio},
};

use anyhow::{anyhow, Result};
use futures::{
    future::ok,
    stream::{iter, StreamExt, TryStreamExt},
};
use tokio::process::Command;

use crate::envs::{alpine_image, base_image, internal, log_path, runner, syntax};

// TODO: tune logging verbosity https://docs.rs/clap-verbosity-flag/latest/clap_verbosity_flag/

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
        ("RUSTCBUILDX_ALPINE_IMAGE", Some(alpine_image().await.to_owned())),
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
    let imgs = [
        (internal::syntax(), syntax().await),
        (internal::base_image(), base_image().await),
        (internal::alpine_image(), alpine_image().await),
    ];

    let mut to_pull = Vec::with_capacity(imgs.len());
    for (user_input, img) in imgs {
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
        to_pull.push(img.to_owned());
        println!("Pulling {img}...");
    }

    let zero = Some(0);
    // TODO: nice TUI that handles concurrent progress
    let code = iter(to_pull.into_iter())
        .map(|img| async move { do_pull(img).await })
        .boxed() // https://github.com/rust-lang/rust/issues/104382
        .buffered(10)
        .try_fold(zero, |a, b| if a == zero { ok(b) } else { ok(a) })
        .await?;
    Ok(exit_code(code))
}

async fn do_pull(img: String) -> Result<Option<i32>> {
    let command = runner();
    let o = Command::new(command)
        .kill_on_drop(true)
        .arg("pull")
        .arg(&img)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("Failed to start `{command} pull {img}`: {e}"))?
        .wait()
        .await
        .map_err(|e| anyhow!("Failed to call `{command} pull {img}`: {e}"))?;
    if !o.success() {
        println!("Failed to pull {img}");
        return Ok(o.code());
    }
    Ok(Some(0)) // TODO: -> Result<ExitCode>, once exit codes impl PartialEq.
}

#[inline]
pub(crate) fn exit_code(code: Option<i32>) -> ExitCode {
    (code.unwrap_or(-1) as u8).into() // TODO: https://doc.rust-lang.org/std/os/unix/process/trait.ExitStatusExt.html
}
