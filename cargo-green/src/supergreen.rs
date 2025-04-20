use core::str;
use std::{collections::BTreeMap, env, io::Cursor, process::Stdio};

use anyhow::{anyhow, bail, Result};
use futures::stream::{iter, StreamExt, TryStreamExt};
use serde_jsonlines::AsyncBufReadJsonLines;
use tokio::io::BufReader;

use crate::{
    cargo_green::{ENV_BUILDER_IMAGE, ENV_FINAL_PATH, ENV_RUNNER, ENV_SYNTAX},
    envs::{cache_image, incremental, internal, log_path},
    extensions::ShowCmd,
    green::{Green, ENV_BASE_IMAGE, ENV_BASE_IMAGE_INLINE},
    runner::runner_cmd,
};

// TODO: tune logging verbosity https://docs.rs/clap-verbosity-flag/latest/clap_verbosity_flag/

// TODO: cargo green cache --keep-less-than=(1month|10GB)      Set $RUSTCBUILDX_CACHE_IMAGE to apply to tagged images.

// TODO: cli for stats (cache hit/miss/size/age/volume, existing available/selected runners, disk usage/free)

pub(crate) async fn main(green: Green, arg1: Option<&str>, args: Vec<String>) -> Result<()> {
    match arg1 {
        None | Some("-h" | "--help" | "-V" | "--version") => help(),
        Some("env") => envs(green, args),
        Some("pull") => return pull(green).await,
        Some("push") => return push(green).await,
        Some(arg) => bail!("Unexpected supergreen command {arg:?}"),
    }
    Ok(())
}

//TODO: util to inspect + clear (+ push) build cache: docker buildx du --verbose
//TODO: prune command (use filters) https://github.com/docker/buildx/pull/2473
//      ~ 🤖 docker buildx du --verbose --filter type=frontend
// ID:     peng2elrcincm360vextha1zz
// Created at: 2025-03-30 16:26:19.48787607 +0000 UTC
// Mutable:    false
// Reclaimable:    true
// Shared:     false
// Size:       0B
// Description:    pulled from docker.io/docker/dockerfile:1@sha256:4c68376a702446fc3c79af22de146a148bc3367e73c25a5803d453b6b3f722fb
// Usage count:    1
// Last used:  2 days ago
// Type:       frontend
//      ~ 🤖 docker buildx du --verbose --filter type=source.git.checkout
// ID:     9serb7k61zusy8vf6x7k4yp2f
// Created at: 2025-03-30 16:26:22.145481847 +0000 UTC
// Mutable:    true
// Reclaimable:    true
// Shared:     false
// Size:       41.58kB
// Description:    git snapshot for https://github.com/fenollp/buildxargs.git#df9b810011cd416b8e3fc02911f2f496acb8475e
// Usage count:    1
// Last used:  2 days ago
// Type:       source.git.checkout

pub(crate) fn help() {
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
}

// TODO: make it work for podman: https://github.com/containers/podman/issues/2369
// TODO: have fun with https://github.com/console-rs/indicatif
async fn push(green: Green) -> Result<()> {
    let Some(img) = cache_image() else { return Ok(()) };
    let img = img.trim_start_matches("docker-image://");
    let tags = all_tags_of(&green, img).await?;

    async fn do_push(green: &Green, tag: String, img: &str) -> Result<()> {
        println!("Pushing {img}:{tag}...");
        let mut cmd = runner_cmd(green);
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

    iter(tags).map(|tag| do_push(&green, tag, img)).buffer_unordered(10).try_collect().await
}

// TODO: test with known tags
// TODO: test docker.io/ prefix bug for the future
async fn all_tags_of(green: &Green, img: &str) -> Result<Vec<String>> {
    // NOTE: https://github.com/moby/moby/issues/47809
    //   Meanwhile: just drop docker.io/ prefix
    let mut cmd = runner_cmd(green);
    cmd.arg("image")
        .arg("ls")
        .arg("--format=json")
        .arg(format!("--filter=reference={}:*", img.trim_start_matches("docker.io/")));
    let o = cmd.output().await.map_err(|e| anyhow!("Failed calling {}: {e}", cmd.show()))?;
    if !o.status.success() {
        bail!("Failed to list tags of image {img}")
    }
    Ok(BufReader::new(Cursor::new(str::from_utf8(&o.stdout).unwrap()))
        .json_lines()
        .filter_map(|x| async move { x.ok() })
        .filter_map(|x: serde_json::Value| async move {
            x.get("Tag").and_then(|x| x.as_str().map(ToOwned::to_owned))
        })
        .collect()
        .await)
}

fn envs(green: Green, vars: Vec<String>) {
    let all: BTreeMap<_, _> = [
        (internal::RUSTCBUILDX, internal::this()),
        (ENV_BASE_IMAGE, Some(green.image.base_image)),
        (ENV_BASE_IMAGE_INLINE, green.image.base_image_inline),
        (ENV_BUILDER_IMAGE, Some(green.builder_image)),
        (internal::RUSTCBUILDX_CACHE_IMAGE, cache_image().to_owned()),
        (internal::RUSTCBUILDX_INCREMENTAL, incremental().then_some("1".to_owned())),
        (internal::RUSTCBUILDX_LOG, internal::log()),
        (internal::RUSTCBUILDX_LOG_PATH, Some(log_path().to_owned())),
        (internal::RUSTCBUILDX_LOG_STYLE, internal::log_style()),
        (ENV_RUNNER, Some(green.runner.clone())),
        (ENV_SYNTAX, Some(green.syntax)),
        (ENV_FINAL_PATH, green.final_path.map(|x| x.to_string())),
        // (ENV_ADD_APK, Some(green.add.apk)), FIXME: CSV
        // (ENV_ADD_APT, Some(green.add.apt)), FIXME: CSV
        // (ENV_ADD_APT_GET, Some(green.add.apt_get)), FIXME: CSV
        // (ENV_SET_ENVS, Some(green.set_envs)), FIXME: CSV
    ]
    .into_iter()
    .collect();

    fn show(var: &str, o: Option<&str>) {
        println!("{var}={}", o.unwrap_or_default());
    }

    let mut empty_vars = true;
    for var in vars {
        if let Some(o) = all.get(&var.as_str()) {
            show(&var, o.as_deref());
            empty_vars = false;
        }
    }
    if empty_vars {
        all.into_iter().for_each(|(var, o)| show(var, o.as_deref()));
    }
}

async fn pull(green: Green) -> Result<()> {
    let imgs = [
        (env::var(ENV_SYNTAX).ok(), green.syntax.as_str()),
        (env::var(ENV_BASE_IMAGE).ok(), green.image.base_image.as_str()),
        (env::var(ENV_BUILDER_IMAGE).ok(), green.builder_image.as_str()),
        //FIXME: also pull xx image if needed
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
    pull_images(green, to_pull).await
}

#[must_use]
fn trim_docker_image(x: &str) -> Option<String> {
    let x = x.trim_start_matches("docker-image://");

    let x =
        if x.contains('@') { x.trim_end_matches(|c| c != '@').trim_end_matches('@') } else { x };

    (!x.is_empty()).then(|| x.to_owned())
}

async fn pull_images(green: Green, to_pull: Vec<String>) -> Result<()> {
    // TODO: nice TUI that handles concurrent progress
    iter(to_pull.into_iter())
        .map(|img| async {
            println!("Pulling {img}...");
            do_pull(&green, img).await
        })
        .buffer_unordered(10)
        .try_collect()
        .await
}

async fn do_pull(green: &Green, img: String) -> Result<()> {
    let mut cmd = runner_cmd(green);
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
