use core::str;
use std::{env, io::Cursor, process::Stdio};

use anyhow::{anyhow, bail, Result};
use futures::stream::{iter, StreamExt, TryStreamExt};
use serde_jsonlines::AsyncBufReadJsonLines;
use tokio::io::BufReader;

use crate::{
    base::{ENV_BASE_IMAGE, ENV_BASE_IMAGE_INLINE, ENV_WITH_NETWORK},
    cargo_green::{ENV_BUILDER_IMAGE, ENV_FINAL_PATH, ENV_RUNNER, ENV_SYNTAX},
    extensions::ShowCmd,
    green::{
        Green, ENV_ADD_APK, ENV_ADD_APT, ENV_ADD_APT_GET, ENV_CACHE_IMAGES, ENV_INCREMENTAL,
        ENV_SET_ENVS,
    },
    logging::{ENV_LOG, ENV_LOG_PATH, ENV_LOG_STYLE},
    rustc_wrapper::ENV,
};

// TODO: tune logging verbosity https://docs.rs/clap-verbosity-flag/latest/clap_verbosity_flag/

// TODO: cargo green cache --keep-less-than=(1month|10GB)      Set $CARGOGREEN_CACHE_IMAGES to apply to tagged images.

// TODO: cli for stats (cache hit/miss/size/age/volume, existing available/selected runners, disk usage/free)

pub(crate) async fn main(green: Green, arg1: Option<&str>, args: Vec<String>) -> Result<()> {
    match arg1 {
        None | Some("-h" | "--help" | "-V" | "--version") => help(),
        Some("env") => envs(green, args),
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
  cargo green fetch                      Pulls images (respects $DOCKER_HOST)
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
    for img in &green.cache_images {
        let img = img.trim_start_matches("docker-image://");
        let tags = all_tags_of(&green, img).await?;

        async fn do_push(green: &Green, tag: String, img: &str) -> Result<()> {
            println!("Pushing {img}:{tag}...");
            let mut cmd = green.runner.as_cmd();
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

        iter(tags)
            .map(|tag| do_push(&green, tag, img))
            .buffer_unordered(10)
            .try_collect::<()>()
            .await?;
    }
    Ok(())
}

// TODO: test with known tags
// TODO: test docker.io/ prefix bug for the future
async fn all_tags_of(green: &Green, img: &str) -> Result<Vec<String>> {
    // NOTE: https://github.com/moby/moby/issues/47809
    //   Meanwhile: just drop docker.io/ prefix
    let mut cmd = green.runner.as_cmd();
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
    let csv = |add: &[String]| (!add.is_empty()).then(|| add.join(","));
    let all = vec![
        (ENV, env::var(ENV).ok()),
        (ENV_ADD_APK, csv(&green.add.apk)),
        (ENV_ADD_APT, csv(&green.add.apt)),
        (ENV_ADD_APT_GET, csv(&green.add.apt_get)),
        (ENV_BASE_IMAGE, Some(green.image.base_image.clone())),
        (ENV_BASE_IMAGE_INLINE, green.image.base_image_inline.clone()),
        (ENV_BUILDER_IMAGE, green.builder_image.clone()),
        (ENV_CACHE_IMAGES, csv(&green.cache_images)),
        (ENV_FINAL_PATH, green.final_path.as_deref().map(ToString::to_string)),
        (ENV_INCREMENTAL, green.incremental.then(|| "1".to_owned())),
        (ENV_LOG, env::var(ENV_LOG).ok()),
        (ENV_LOG_PATH, env::var(ENV_LOG_PATH).ok()),
        (ENV_LOG_STYLE, env::var(ENV_LOG_STYLE).ok()),
        (ENV_RUNNER, Some(green.runner.to_string())),
        (ENV_SET_ENVS, csv(&green.set_envs)),
        (ENV_SYNTAX, Some(green.syntax)),
        (ENV_WITH_NETWORK, Some(green.image.with_network.to_string())),
    ];

    let mut empty_vars = true;
    for var in vars {
        if let Some(o) = all.iter().find_map(|(k, v)| (k == &var).then_some(v)) {
            println!("{}", o.as_deref().unwrap_or_default());
            empty_vars = false;
        }
    }
    if empty_vars {
        all.into_iter().for_each(|(var, o)| println!("{var}={:?}", o.unwrap_or_default()));
    }
}
