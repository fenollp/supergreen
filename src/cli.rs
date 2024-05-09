use std::{
    collections::BTreeMap,
    env,
    io::Cursor,
    process::{ExitCode, Stdio},
};

use anyhow::{anyhow, Error, Result};
use futures::{
    future::ok,
    stream::{iter, StreamExt, TryStreamExt},
};
use serde_jsonlines::AsyncBufReadJsonLines;
use tokio::{io::BufReader, process::Command};

use crate::{
    envs::{base_image, cache_image, internal, log_path, runner, syntax},
    extensions::ShowCmd,
};

// TODO: tune logging verbosity https://docs.rs/clap-verbosity-flag/latest/clap_verbosity_flag/

#[inline]
pub(crate) fn help() -> ExitCode {
    println!(
        "{name}@{version}: {description}
    {repository}

Usage:
  {name} env             Show used values
  {name} pull            Pulls images (respects $DOCKER_HOST)
  {name} push            Push cache image (all tags)
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

// TODO: make it work for podman: https://github.com/containers/podman/issues/2369
pub(crate) async fn push() -> Result<ExitCode> {
    if let Some(img) = cache_image() {
        let img = img.trim_start_matches("docker-image://");

        let tags = match all_tags_of(img).await? {
            (_, Some(code)) => return Ok(code),
            (tags, _) => tags,
        };

        iter(tags)
            .map(|tag: String| async move {
                println!("Pushing {img}:{tag}...");
                let mut cmd = Command::new(runner());
                let cmd = cmd
                    .kill_on_drop(true)
                    .arg("push")
                    .arg(format!("{img}:{tag}"))
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                let o = cmd
                    .spawn()
                    .map_err(|e| anyhow!("Failed to start {}: {e}", cmd.show()))?
                    .wait()
                    .await
                    .map_err(|e| anyhow!("Failed to call {}: {e}", cmd.show()))?;
                if !o.success() {
                    eprintln!("Pushing {img}:{tag} failed!");
                } else {
                    println!("Pushing {img}:{tag}... done!");
                }
                Ok(0)
            })
            .buffered(10)
            .try_fold(0, |a, b| ok::<_, Error>(a + b))
            .await?;
    }
    Ok(exit_code(Some(0)))
}

// TODO: test with known tags
// TODO: test docker.io/ prefix bug for the future
async fn all_tags_of(img: &str) -> Result<(Vec<String>, Option<ExitCode>)> {
    //thats a silly return type

    // NOTE: https://github.com/moby/moby/issues/47809
    //   Meanwhile: just drop docker.io/ prefix
    let mut cmd = Command::new(runner());
    let cmd = cmd
        .kill_on_drop(true)
        .arg("image")
        .arg("ls")
        .arg("--format=json")
        .arg(format!("--filter=reference={}:*", img.trim_start_matches("docker.io/")));
    let o = cmd.output().await.map_err(|e| anyhow!("Failed calling {}: {e}", cmd.show()))?;
    if !o.status.success() {
        eprintln!("Failed to list tags of image {img}");
        return Ok((vec![], Some(exit_code(o.status.code()))));
    }
    let tags: Vec<String> = BufReader::new(Cursor::new(String::from_utf8(o.stdout).unwrap()))
        .json_lines()
        .filter_map(|x| async move { x.ok() })
        .filter_map(|x: serde_json::Value| async move {
            x.get("Tag").and_then(|x| x.as_str().map(ToOwned::to_owned))
        })
        .collect()
        .await;

    Ok((tags, None))
}

pub(crate) async fn envs(vars: Vec<String>) -> ExitCode {
    let all: BTreeMap<_, _> = [
        ("RUSTCBUILDX", internal::this()),
        ("RUSTCBUILDX_BASE_IMAGE", Some(base_image().await.to_owned())),
        ("RUSTCBUILDX_CACHE_IMAGE", cache_image().to_owned()),
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
    let imgs = [(internal::syntax(), syntax().await), (internal::base_image(), base_image().await)];

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
    let mut cmd = Command::new(runner());
    let cmd = cmd.kill_on_drop(true).arg("pull").arg(&img).stdin(Stdio::null());
    let o = cmd
        .spawn()
        .map_err(|e| anyhow!("Failed to start {}: {e}", cmd.show()))?
        .wait()
        .await
        .map_err(|e| anyhow!("Failed to call {}: {e}", cmd.show()))?;
    if !o.success() {
        eprintln!("Failed to pull {img}");
        return Ok(o.code());
    }
    Ok(Some(0)) // TODO: -> Result<ExitCode>, once exit codes impl PartialEq.
}

#[inline]
pub(crate) fn exit_code(code: Option<i32>) -> ExitCode {
    (code.unwrap_or(-1) as u8).into() // TODO: https://doc.rust-lang.org/std/os/unix/process/trait.ExitStatusExt.html
}
