use core::str;
use std::{env, io::Cursor, process::Stdio};

use anyhow::{bail, Result};
use futures::stream::{iter, StreamExt, TryStreamExt};
use serde_jsonlines::AsyncBufReadJsonLines;
use tokio::io::BufReader;

use crate::{ext::CommandExt, green::Green, image_uri::ImageUri, PKG, REPO, VSN};

// TODO: tune logging verbosity https://docs.rs/clap-verbosity-flag/latest/clap_verbosity_flag/

// TODO: cargo green cache --keep-less-than=(1month|10GB)      Set $CARGOGREEN_CACHE_IMAGES to apply to tagged images.

// TODO: cli for stats (cache hit/miss/size/age/volume, existing available/selected runners, disk usage/free)

//TODO: cli to show cfg'd builder

//TODO: cli shows builder's jaeger: BUILDX_BUILDER=supergreen docker buildx history trace --addr 127.0.0.1:5452

// # With this, one may also use this set of subcommands: [UNSTABLE API] (refacto into a `cache` cmd)
// cargo supergreen config get   VAR*
// cargo supergreen config set   VAR VAL
// cargo supergreen config unset VAR
// cargo supergreen pull-images             Pulls latest versions of images used for the build, no cache (respects $DOCKER_HOST)
// cargo supergreen pull-cache              Pulls all from `--cache-from`
// cargo supergreen push-cache              Pushes all to `--cache-to`

pub(crate) async fn main(green: Green, arg1: Option<&str>, args: Vec<String>) -> Result<()> {
    if just_help(arg1) {
        help();
        return Ok(());
    }
    match arg1 {
        Some("env") => envs(green, args)?,
        Some("doc") => docs(green, args)?,
        Some("push") => return push(green).await,
        Some(arg) => bail!("Unexpected supergreen command {arg:?}"),
        None => unreachable!(),
    }
    Ok(())
}

pub(crate) fn just_help(arg1: Option<&str>) -> bool {
    matches!(arg1, None | Some("-h" | "--help" | "-V" | "--version"))
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
        "{PKG} v{VSN}

        {description}

    {REPO}

{usage}
",
        description = env!("CARGO_PKG_DESCRIPTION"),
        usage = include_str!("../docs/usage.md").trim().replace("```shell", "").replace("```", ""),
    );
}

// TODO: make it work for podman: https://github.com/containers/podman/issues/2369
// TODO: have fun with https://github.com/console-rs/indicatif
async fn push(green: Green) -> Result<()> {
    for img in &green.cache_images {
        let img = img.noscheme();
        let tags = all_tags_of(&green, img).await?;

        async fn do_push(green: &Green, tag: String, img: &str) -> Result<()> {
            println!("Pushing {img}:{tag}...");
            let mut cmd = green.cmd()?;
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
    let mut cmd = green.cmd()?;
    cmd.args(["image", "ls", "--format=json"]);
    cmd.arg(format!("--filter=reference={}:*", img.trim_start_matches("docker.io/")));

    let (succeeded, stdout, stderr) = cmd.exec().await?;
    if !succeeded {
        let stderr = String::from_utf8_lossy(&stderr);
        bail!("Failed to list tags of image {img}: {stderr}")
    }
    let stdout = String::from_utf8_lossy(&stdout);

    Ok(BufReader::new(Cursor::new(stdout.to_string()))
        .json_lines()
        .filter_map(|x| async move { x.ok() })
        .filter_map(|x: serde_json::Value| async move {
            x.get("Tag").and_then(|x| x.as_str().map(ToOwned::to_owned))
        })
        .collect()
        .await)
}

fn csv(xs: &[String]) -> Option<String> {
    (!xs.is_empty()).then(|| xs.join(","))
}

fn csv_uris(xs: &[ImageUri]) -> Option<String> {
    csv(&xs.iter().map(ToString::to_string).collect::<Vec<_>>())
}

macro_rules! var {
    ($env:expr, $repr:expr) => {
        ($env, include_str!(concat!("../docs/", $env, ".md")), $repr)
    };
}

fn all_envs(green: &Green) -> Vec<(&str, &'static str, Option<String>)> {
    let builder_name = || {
        green
            .builder
            .name
            .as_deref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "supergreen".to_owned())
    };
    vec![
        // var!(ENV!(), env::var(ENV!()).ok()),
        var!(ENV_LOG_PATH!(), env::var(ENV_LOG_PATH!()).ok()),
        var!(ENV_LOG!(), env::var(ENV_LOG!()).ok()),
        var!(ENV_LOG_STYLE!(), env::var(ENV_LOG_STYLE!()).ok()),
        var!(ENV_RUNNER!(), Some(green.runner.to_string())),
        var!(BUILDX_BUILDER!(), Some(builder_name())),
        var!(ENV_BUILDER_IMAGE!(), green.builder.image.as_deref().map(ToString::to_string)),
        var!(ENV_SYNTAX_IMAGE!(), Some(green.syntax.to_string())),
        var!(ENV_REGISTRY_MIRRORS!(), csv(&green.registry_mirrors)),
        var!(ENV_CACHE_IMAGES!(), csv_uris(&green.cache_images)),
        var!(ENV_CACHE_FROM_IMAGES!(), csv_uris(&green.cache_from_images)),
        var!(ENV_CACHE_TO_IMAGES!(), csv_uris(&green.cache_to_images)),
        var!(ENV_FINAL_PATH!(), green.final_path.as_deref().map(ToString::to_string)),
        var!(ENV_FINAL_PATH_NONPRIMARY!(), green.final_path_nonprimary.then(|| "1".to_owned())),
        var!(ENV_BASE_IMAGE!(), Some(green.image.base_image.to_string())),
        var!(ENV_SET_ENVS!(), csv(&green.set_envs)),
        var!(ENV_BASE_IMAGE_INLINE!(), green.image.base_image_inline.clone()),
        var!(ENV_WITH_NETWORK!(), Some(green.image.with_network.to_string())),
        var!(ENV_ADD_APT!(), csv(&green.add.apt)),
        var!(ENV_ADD_APT_GET!(), csv(&green.add.apt_get)),
        var!(ENV_ADD_APK!(), csv(&green.add.apk)),
        var!(ENV_INCREMENTAL!(), green.incremental.then(|| "1".to_owned())),
    ]
}

fn for_all_or_filtered(
    green: &Green,
    vars: Vec<String>,
    f: fn(&str, &'static str, Option<&str>),
) -> Result<()> {
    let mut envs = all_envs(green);
    if vars.is_empty() {
        for (k, doc, v) in envs {
            f(k, doc, v.as_deref())
        }
        return Ok(());
    }

    envs.retain(|(k, _, _)| vars.contains(&(*k).to_owned()));
    for var in vars {
        let Some((k, doc, v)) = envs.iter().find(|(k, _, _)| *k == var) else {
            bail!("Unexpected env {var}")
        };
        f(k, doc, v.as_deref())
    }

    Ok(())
}

fn envs(green: Green, vars: Vec<String>) -> Result<()> {
    for_all_or_filtered(&green, vars, |var: &str, _doc: &'static str, val: Option<&str>| {
        println!("{var}={:?}", val.unwrap_or_default());
    })
}

fn docs(green: Green, vars: Vec<String>) -> Result<()> {
    for_all_or_filtered(&green, vars, |var: &str, doc: &'static str, val: Option<&str>| {
        println!();
        termimad::print_text(&format!("# ${var}"));
        if let Some(val) = val {
            let val = val.trim().lines().collect::<Vec<_>>().join("\n> ");
            termimad::print_text(&format!("> {val}"));
            println!();
        }
        termimad::print_text(doc);
    })
}
