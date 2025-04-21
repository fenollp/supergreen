use std::{
    env,
    fs::{self, OpenOptions},
    process::{Output, Stdio},
};

use anyhow::{anyhow, bail, Result};
use camino::absolute_utf8;
use futures::{stream::iter, StreamExt, TryStreamExt};
use log::{debug, info, trace};
use tokio::{process::Command, try_join};

use crate::{
    cratesio::{self},
    envs::{cache_image, internal},
    extensions::ShowCmd,
    green::{Green, ENV_BASE_IMAGE},
    hash, hashed_args,
    lockfile::{find_lockfile, locked_crates},
    logging::{self, maybe_log, ENV_LOG, ENV_LOG_PATH},
    pwd,
    runner::{build_cacheonly, fetch_digest, maybe_lock_image, runner_cmd},
    stage::{Stage, RST, RUST},
    tmp, PKG, REPO, VSN,
};

// Env-only settings (no Cargo.toml equivalent setting)
pub(crate) const ENV_BUILDER_IMAGE: &str = "CARGOGREEN_BUILDER_IMAGE";
pub(crate) const ENV_FINAL_PATH: &str = "CARGOGREEN_FINAL_PATH";
pub(crate) const ENV_RUNNER: &str = "CARGOGREEN_RUNNER";
pub(crate) const ENV_SYNTAX: &str = "CARGOGREEN_SYNTAX";

pub(crate) async fn main(cmd: &mut Command) -> Result<Green> {
    // TODO: TUI above cargo output (? https://docs.rs/prodash )

    if let Ok(log) = env::var(ENV_LOG) {
        cmd.env(ENV_LOG, log);
        let path = env::var(ENV_LOG_PATH)
            .unwrap_or_else(|_| tmp().join(format!("{PKG}-{}.log", hashed_args())).to_string());
        env::set_var(ENV_LOG_PATH, &path);
        cmd.env(ENV_LOG_PATH, &path);
        let _ = OpenOptions::new().create(true).truncate(false).append(true).open(path);
    }

    // RUSTCBUILDX and eponymous envs are handled by wrapper
    assert!(env::var_os("RUSTCBUILDX").is_none());

    // This exports vars so they will be accessible by later-spawned $RUSTC_WRAPPER.
    // TODO: separate env reading from env setting.
    // TODO: get from ENV..SETTINGS, avoid most env setting.

    // Goal: produce only fully-locked Dockerfiles/TOMLs

    assert!(env::var_os("CARGOGREEN").is_none());
    env::set_var("CARGOGREEN", "1");

    let mut green = Green::new_from_env_then_manifest()?;

    // Setting runner first as it's needed by many calls
    if !green.runner.is_empty() {
        bail!("${ENV_RUNNER} can only be set through the environment variable")
    }
    green.runner = env::var(ENV_RUNNER).unwrap_or_else(|_| "docker".to_owned());
    if !["docker", "none", "podman"].contains(&green.runner.as_str()) {
        bail!("${ENV_RUNNER} must be either 'docker', 'podman' or 'none' not {:?}", green.runner)
    }

    const SYNTAX: &str = "docker-image://docker.io/docker/dockerfile:1";
    if !green.syntax.is_empty() {
        bail!("${ENV_SYNTAX} can only be set through the environment variable")
    }
    green.syntax = env::var(ENV_SYNTAX).unwrap_or_else(|_| SYNTAX.to_owned());
    // Use local hashed image if one matching exists locally
    green.syntax = maybe_lock_image(&green, &green.syntax).await;
    // otherwise default to a hash found through some Web API
    green.syntax = fetch_digest(&green.syntax).await?;
    if !green.syntax.starts_with(SYNTAX) {
        // Enforce a known stable syntax + allow pinning to digest
        bail!("${ENV_SYNTAX} must be a digest of {SYNTAX}")
    }

    if green.builder_image.is_some() {
        bail!("${ENV_BUILDER_IMAGE} can only be set through the environment variable")
    }
    if let Ok(builder_image) = env::var(ENV_BUILDER_IMAGE) {
        let builder_image = maybe_lock_image(&green, &builder_image).await;
        green.builder_image = Some(fetch_digest(&builder_image).await?);
    }
    if let Some(ref builder_image) = green.builder_image {
        if !builder_image.starts_with("docker-image://") {
            bail!("${ENV_BUILDER_IMAGE} must be a docker-image://")
        }
    }

    if green.final_path.is_some() {
        bail!("${ENV_FINAL_PATH} can only be set through the environment variable")
    }
    // TODO? provide a way to export final as flatpack
    if let Ok(path) = env::var(ENV_FINAL_PATH) {
        if path.is_empty() {
            bail!("${ENV_FINAL_PATH} is empty")
        }
        if path == "-" {
            bail!("${ENV_FINAL_PATH} must not be {path:?}")
        }
        let path = absolute_utf8(path)
            .map_err(|e| anyhow!("Failed canonicalizing ${ENV_FINAL_PATH}: {e}"))?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| anyhow!("Failed `mkdir -p {dir}`: {e}"))?;
        }
        green.final_path = Some(path);
    }

    if !green.image.base_image.contains('@') {
        let mut base = maybe_lock_image(&green, &green.image.base_image).await;
        base = fetch_digest(&base).await?;
        green.image = green.image.lock_base_to(base);
    }

    let (mut with_network, mut finalized_block) = green.image.as_block();
    if !green.add.is_empty() {
        (with_network, finalized_block) = green.add.as_block(&finalized_block);
    }
    green.image.with_network = with_network;
    green.image.base_image_inline = Some(finalized_block.trim().to_owned());

    // don't pull
    if let Some(val) = cache_image() {
        cmd.env(internal::RUSTCBUILDX_CACHE_IMAGE, val);
        //TODO: $CARGOGREEN_IMAGE_CACHES? (comma separated)
    }

    // FIXME "multiplex conns to daemon" https://github.com/docker/buildx/issues/2564#issuecomment-2207435201
    // > If you do have docker context created already on ssh endpoint then you don't need to set the ssh address again on buildx create, you can use the context name or let it use the active context.

    // https://linuxhandbook.com/docker-remote-access/
    // https://thenewstack.io/connect-to-remote-docker-machines-with-docker-context/
    // https://www.cyberciti.biz/faq/linux-unix-reuse-openssh-connection/
    // https://github.com/moby/buildkit/issues/4268#issuecomment-2128464135
    // https://github.com/moby/buildkit/blob/v0.15.1/session/sshforward/sshprovider/agentprovider.go#L119

    // https://crates.io/crates/async-ssh2-tokio
    // https://crates.io/crates/russh

    // https://docs.docker.com/build/building/variables/#buildx_builder
    if let Ok(ctx) = env::var("DOCKER_HOST") {
        info!("$DOCKER_HOST is set to {ctx:?}");
        eprintln!("$DOCKER_HOST is set to {ctx:?}");
    } else if let Ok(ctx) = env::var("BUILDX_BUILDER") {
        info!("$BUILDX_BUILDER is set to {ctx:?}");
        eprintln!("$BUILDX_BUILDER is set to {ctx:?}");
    } else if let Ok(remote) = env::var("CARGOGREEN_REMOTE") {
        //     // docker buildx create \
        //     //   --name supergreen \
        //     //   --driver remote \
        //     //   tcp://localhost:1234
        //     //{remote}
        //     env::set_var("DOCKER_CONTEXT", "supergreen"); //FIXME: ensure this gets passed down & used
        panic!("$CARGOGREEN_REMOTE is reserved but set to: {remote}");
    } else if false {
        setup_build_driver(&green, "supergreen").await?; // FIXME? maybe_..
        env::set_var("BUILDX_BUILDER", "supergreen");

        // TODO? docker dial-stdio proxy
        // https://github.com/docker/cli/blob/9bb1a62735174e9220d84fecc056a0ef8a1fc26f/cli/command/system/dial_stdio.go

        // https://docs.docker.com/engine/context/working-with-contexts/
        // https://docs.docker.com/engine/security/protect-access/
    }

    Ok(green)
}

// https://docs.docker.com/build/drivers/docker-container/
// https://docs.docker.com/build/drivers/remote/
// https://docs.docker.com/build/drivers/kubernetes/
async fn setup_build_driver(green: &Green, name: &str) -> Result<()> {
    if false {
        // TODO: reuse old state but try auto-upgrading builder impl
        try_removing_previous_builder(green, name).await;
    }

    let Some(ref builder_image) = green.builder_image else { return Ok(()) };
    let builder_image = builder_image.trim_start_matches("docker-image://");

    let mut cmd = runner_cmd(green);
    cmd.args(["buildx", "create"])
        .arg(format!("--name={name}"))
        .arg("--bootstrap")
        .arg("--driver=docker-container")
        .arg(format!("--driver-opt=image={builder_image}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let call = cmd.show();
    let envs: Vec<_> = cmd.as_std().get_envs().map(|(k, v)| format!("{k:?}={v:?}")).collect();
    let envs = envs.join(" ");

    info!("Calling {call} (env: {envs:?})`");
    eprintln!("Calling {call} (env: {envs:?})`");

    let Output { status, stderr, .. } = cmd.output().await?;
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr);
        if !stderr.starts_with(r#"ERROR: existing instance for "supergreen""#) {
            bail!("BUG: failed to create builder: {stderr}")
        }
    }

    Ok(())
}

async fn try_removing_previous_builder(green: &Green, name: &str) {
    let mut cmd = runner_cmd(green);
    cmd.args(["buildx", "rm", name, "--keep-state", "--force"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let call = cmd.show();
    let envs: Vec<_> = cmd.as_std().get_envs().map(|(k, v)| format!("{k:?}={v:?}")).collect();
    let envs = envs.join(" ");

    info!("Calling {call} (env: {envs:?})`");
    eprintln!("Calling {call} (env: {envs:?})`");

    let _ = cmd.status().await;
}

pub(crate) async fn maybe_prebuild_base(green: &Green) -> Result<()> {
    let syntax = green.syntax.trim_start_matches("docker-image://");
    let mut header = format!("# syntax={syntax}\n");
    header.push_str("# check=error=true\n");
    header.push_str(&format!("# Generated by {REPO} v{VSN}\n"));
    header.push('\n');
    header.push_str(green.image.base_image_inline.as_deref().unwrap());
    header.push('\n');

    let dockerfile_path = tmp().join(format!("{PKG}-{RST}-{}.Dockerfile", hash(&header)));
    if dockerfile_path.exists() {
        return Ok(());
    }
    fs::write(&dockerfile_path, &header)
        .map_err(|e| anyhow!("Failed creating dockerfile {dockerfile_path}: {e}"))?;

    // Turns out --network is part of BuildKit's cache key, so an initial online build
    // won't cache hit on later offline builds.
    build_cacheonly(green, &dockerfile_path, RUST.clone()).await.map_err(|e| {
        // TODO: catch ^C (and co.) to make sure file gets removed
        let _ = fs::remove_file(&dockerfile_path);
        anyhow!("{header}\n\nUnable to build {RST}: {e}")
    })
}

pub(crate) async fn fetch(green: Green) -> Result<()> {
    logging::setup("fetch");
    let _ = maybe_log();
    info!("{PKG}@{VSN} original args: {:?} pwd={:?}", env::args(), pwd());

    let manifest_path_lockfile = find_lockfile().await?;
    debug!("using lockfile at {manifest_path_lockfile}");

    let packages = locked_crates(&manifest_path_lockfile).await?;
    info!("found {} packages", packages.len());
    if packages.is_empty() {
        return Ok(());
    }

    let imgs = vec![
        (env::var(ENV_SYNTAX).ok(), Some(green.syntax.as_str())),
        (env::var(ENV_BASE_IMAGE).ok(), Some(green.image.base_image.as_str())),
        (env::var(ENV_BUILDER_IMAGE).ok(), green.builder_image.as_deref()),
    ]; // NOTE: we don't pull cache_image()

    let dockerfile_path = {
        let pkgs: Vec<_> = packages.iter().map(|(n, v, h)| format!("{n} {v} {h}\n")).collect();
        let hashed = pkgs.join(" ") + format!("{imgs:?}").as_str();
        let hashed = hash(&hashed);
        tmp().join(format!("{PKG}-fetch-{hashed}.Dockerfile"))
    };

    info!("checking the existence of {dockerfile_path}");
    if dockerfile_path.exists() {
        return Ok(());
    }

    let stage = Stage::try_new("cargo-fetch")?;
    let stager = |i| format!("{stage}-{i}");

    let syntax = green.syntax.trim_start_matches("docker-image://");
    let mut dockerfile = format!("# syntax={syntax}\n");
    dockerfile.push_str("# check=error=true\n");
    dockerfile.push_str(&format!("# Generated by {REPO} v{VSN}\n"));
    dockerfile.push('\n');

    let mut leaves = 0;
    // 127: https://github.com/docker/docs/issues/8230
    for (i, pkgs) in packages.chunks(127).enumerate() {
        leaves = i;

        dockerfile.push_str(&format!("FROM scratch AS {}\n", stager(i)));

        let (name, version, hash) = &pkgs[0];
        debug!("will fetch crate {name}: {version}");
        dockerfile.push_str(&cratesio::add_step(name, version, hash));
        dockerfile.push('\n');

        for (name, version, hash) in &pkgs[1..] {
            debug!("will fetch crate {name}: {version}");
            dockerfile.push_str(&cratesio::add_step(name, version, hash));
            dockerfile.push('\n');
        }
    }
    dockerfile.push_str(&format!("FROM scratch AS {stage}\n"));
    for leaf in 0..=leaves {
        dockerfile.push_str(&format!("COPY --from={} / /\n", stager(leaf)));
    }

    fs::write(&dockerfile_path, dockerfile)
        .map_err(|e| anyhow!("Failed creating dockerfile {dockerfile_path}: {e}"))?;

    let ignore = format!("{dockerfile_path}.dockerignore");
    fs::write(&ignore, "").map_err(|e| anyhow!("Failed creating dockerignore {ignore}: {e}"))?;

    info!("dockerfile: {dockerfile_path}");
    match fs::read_to_string(&dockerfile_path) {
        Ok(data) => data,
        Err(e) => e.to_string(),
    }
    .lines()
    .filter(|x| !x.is_empty())
    .for_each(|line| trace!("❯ {line}"));

    let ((), ()) = try_join!(
        pull(&green, imgs), // NOTE: can't pull these with build(..): they won't get --load'ed
        build_cacheonly(&green, &dockerfile_path, stage)
    )?;
    Ok(())
}

async fn pull(green: &Green, imgs: Vec<(Option<String>, Option<&str>)>) -> Result<()> {
    let mut to_pull = vec![];
    for (user_input, img) in imgs {
        let Some(img) = img else { continue };
        let img = img.trim_start_matches("docker-image://");
        let img = if img.contains('@') && user_input.map(|x| !x.contains('@')).unwrap_or(true) {
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

async fn pull_images(green: &Green, to_pull: Vec<String>) -> Result<()> {
    // TODO: nice TUI that handles concurrent progress
    iter(to_pull.into_iter())
        .map(|img| async {
            println!("Pulling {img}...");
            do_pull(green, img).await
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
