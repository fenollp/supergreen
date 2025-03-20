use std::{collections::BTreeMap, env, io::Cursor, process::Stdio};

use anyhow::{anyhow, bail, Result};
use futures::stream::{iter, StreamExt, TryStreamExt};
use serde_jsonlines::AsyncBufReadJsonLines;
use tokio::io::BufReader;

use crate::{
    envs::{
        base_image, builder_image, cache_image, incremental, internal, log_path, runner, syntax,
    },
    extensions::ShowCmd,
    runner::runner_cmd,
};

// TODO: tune logging verbosity https://docs.rs/clap-verbosity-flag/latest/clap_verbosity_flag/

// TODO: cargo green cache --keep-less-than=(1month|10GB)      Set $RUSTCBUILDX_CACHE_IMAGE to apply to tagged images.

pub(crate) async fn main(arg1: Option<&str>, args: Vec<String>) -> Result<()> {
    match arg1 {
        None | Some("-h" | "--help" | "-V" | "--version") => help(),
        Some("env") => envs(args).await,
        Some("pull") => pull().await,
        Some("push") => push().await,
        Some(arg) => {
            bail!("Unexpected supergreen command {arg:?}")
        }
    }
}

#[expect(clippy::unnecessary_wraps)]
pub(crate) fn help() -> Result<()> {
    println!(
        "{name} v{version}

        {description}

    {repository}

Usage:
  cargo green supergreen env             Show used values
  cargo green supergreen pull            Pulls images (respects $DOCKER_HOST)
  cargo green supergreen push            Push cache image (all tags)
  cargo green supergreen -h | --help
  cargo green supergreen -V | --version
",
        name = env!("CARGO_PKG_NAME"),
        version = env!("CARGO_PKG_VERSION"),
        repository = env!("CARGO_PKG_REPOSITORY"),
        description = env!("CARGO_PKG_DESCRIPTION"),
    );
    Ok(())
}

// TODO: make it work for podman: https://github.com/containers/podman/issues/2369
// TODO: have fun with https://github.com/console-rs/indicatif
async fn push() -> Result<()> {
    let Some(img) = cache_image() else { return Ok(()) };
    let img = img.trim_start_matches("docker-image://");
    let tags = all_tags_of(img).await?;

    async fn do_push(tag: String, img: &str) -> Result<()> {
        println!("Pushing {img}:{tag}...");
        let mut cmd = runner_cmd();
        cmd.arg("push").arg(format!("{img}:{tag}")).stdout(Stdio::null()).stderr(Stdio::null());

        if let Ok(mut o) = cmd.spawn() {
            if let Ok(o) = o.wait().await {
                if o.success() {
                    println!("Pushing {img}:{tag}... done!");
                    return Ok(());
                }
            }
        }
        bail!("Pushing {img}:{tag} failed!")
    }

    iter(tags).map(|tag| do_push(tag, img)).buffer_unordered(10).try_collect().await
}

// TODO: test with known tags
// TODO: test docker.io/ prefix bug for the future
async fn all_tags_of(img: &str) -> Result<Vec<String>> {
    // NOTE: https://github.com/moby/moby/issues/47809
    //   Meanwhile: just drop docker.io/ prefix
    let mut cmd = runner_cmd();
    cmd.arg("image")
        .arg("ls")
        .arg("--format=json")
        .arg(format!("--filter=reference={}:*", img.trim_start_matches("docker.io/")));
    let o = cmd.output().await.map_err(|e| anyhow!("Failed calling {}: {e}", cmd.show()))?;
    if !o.status.success() {
        bail!("Failed to list tags of image {img}")
    }
    Ok(BufReader::new(Cursor::new(String::from_utf8(o.stdout).unwrap()))
        .json_lines()
        .filter_map(|x| async move { x.ok() })
        .filter_map(|x: serde_json::Value| async move {
            x.get("Tag").and_then(|x| x.as_str().map(ToOwned::to_owned))
        })
        .collect()
        .await)
}

async fn envs(vars: Vec<String>) -> Result<()> {
    let all: BTreeMap<_, _> = [
        (internal::RUSTCBUILDX, internal::this()),
        (internal::RUSTCBUILDX_BASE_IMAGE, Some(base_image().await.base())),
        (internal::RUSTCBUILDX_BUILDER_IMAGE, Some(builder_image().await.to_owned())),
        (internal::RUSTCBUILDX_CACHE_IMAGE, cache_image().to_owned()),
        (internal::RUSTCBUILDX_INCREMENTAL, incremental().then_some("1".to_owned())),
        (internal::RUSTCBUILDX_LOG, internal::log()),
        (internal::RUSTCBUILDX_LOG_PATH, Some(log_path().to_owned())),
        (internal::RUSTCBUILDX_LOG_STYLE, internal::log_style()),
        (internal::RUSTCBUILDX_RUNNER, Some(runner().to_owned())),
        (internal::RUSTCBUILDX_SYNTAX, Some(syntax().await.to_owned())),
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

    Ok(())
}

async fn pull() -> Result<()> {
    let imgs = [
        (internal::syntax(), syntax().await),
        (internal::base_image(), &base_image().await.base()),
        (internal::builder_image(), builder_image().await),
    ]; // NOTE: we don't pull cache_image()

    let mut to_pull = Vec::with_capacity(imgs.len());
    for (user_input, img) in imgs {
        let img = img.trim_start_matches("docker-image://");
        let img = if img.contains('@') && user_input.map(|x| !x.contains('@')).unwrap_or_default() {
            // Don't pull a locked image unless that's what's asked
            // Otherwise, pull unlocked

            // The only possible cases (user_input sets img)
            // none + @ = trim
            // none + _ = _
            // s @  + @ = _
            // s !  + @ = trim
            trim_docker_image(img).expect("contains @")
        } else {
            img.to_owned()
        };
        to_pull.push(img);
    }
    pull_images(to_pull).await
}

#[must_use]
fn trim_docker_image(x: &str) -> Option<String> {
    let x = x.trim_start_matches("docker-image://");
    let x = x
        .contains('@')
        .then(|| x.trim_end_matches(|c| c != '@').trim_end_matches('@'))
        .unwrap_or(x);
    (!x.is_empty()).then(|| x.to_owned())
}

async fn pull_images(to_pull: Vec<String>) -> Result<()> {
    // TODO: nice TUI that handles concurrent progress
    iter(to_pull.into_iter())
        .map(|img| async move {
            println!("Pulling {img}...");
            do_pull(img).await
        })
        .buffer_unordered(10)
        .try_collect()
        .await
}

async fn do_pull(img: String) -> Result<()> {
    let mut cmd = runner_cmd();
    cmd.arg("pull").arg(&img);
    let o = cmd
        .spawn()
        .map_err(|e| anyhow!("Failed to start {}: {e}", cmd.show()))?
        .wait()
        .await
        .map_err(|e| anyhow!("Failed to call {}: {e}", cmd.show()))?;
    if !o.success() {
        bail!("Failed to pull {img}")
    }
    Ok(())
}
