use std::{
    env,
    fs::{self},
};

use anyhow::{anyhow, bail, Result};
use futures::{stream::iter, StreamExt, TryStreamExt};
use log::{debug, info, warn};
use tokio::try_join;

use crate::{
    build::fetch_digest,
    cratesio::{self},
    ext::CommandExt,
    green::Green,
    hash,
    image_uri::{ImageUri, SYNTAX_IMAGE},
    lockfile::{find_lockfile, locked_crates},
    logging::{self, maybe_log},
    network::Network,
    pwd,
    runner::{Runner, BUILDKIT_HOST, DOCKER_BUILDKIT, DOCKER_CONTEXT, DOCKER_HOST},
    stage::{Stage, RST, RUST},
    tmp, ENV_FINAL_PATH, ENV_FINAL_PATH_NONPRIMARY, ENV_RUNNER, ENV_SYNTAX_IMAGE, PKG, VSN,
};

pub(crate) async fn main() -> Result<Green> {
    let mut green = Green::new_from_env_then_manifest()?;

    // Setting runner first as it's needed by many calls
    let mut var = ENV_RUNNER!();
    if green.runner != Runner::default() {
        bail!("${var} can only be set through the environment variable")
    }
    if let Ok(val) = env::var(var) {
        green.runner = val.parse().map_err(|e| anyhow!("${var}={val:?} {e}"))?;
    }

    // Read runner's envs only once and disallow conf overrides
    if !green.runner_envs.is_empty() {
        bail!("'runner_envs' setting cannot be set")
    }
    green.runner_envs = green.runner.envs();

    // Cf. https://docs.docker.com/build/buildkit/#getting-started
    if green.runner_envs.get(DOCKER_BUILDKIT).is_some_and(|x| x != "1") {
        bail!("This requires ${DOCKER_BUILDKIT}=1")
    }

    // Cf. https://docs.docker.com/engine/security/protect-access/
    if let Some(val) = green.runner_envs.get(DOCKER_HOST) {
        info!("${DOCKER_HOST} is set to {val:?}");
        eprintln!("${DOCKER_HOST} is set to {val:?}");
    }

    // Cf. https://docs.docker.com/reference/cli/docker/#environment-variables
    if let Some(val) = green.runner_envs.get(DOCKER_CONTEXT) {
        info!("${DOCKER_CONTEXT} is set to {val:?}");
        eprintln!("${DOCKER_CONTEXT} is set to {val:?}");
    }

    // Cf. https://docs.docker.com/build/building/variables/#buildkit_host
    let buildkit_host = green.runner_envs.get(BUILDKIT_HOST);
    if let Some(val) = buildkit_host {
        info!("${BUILDKIT_HOST} is set to {val:?}");
        eprintln!("${BUILDKIT_HOST} is set to {val:?}");
    }

    var = BUILDX_BUILDER!();
    if green.builder.name.is_some() {
        bail!("builder-name can only be set through the environment variable")
    }
    let builder = green.runner_envs.get(var);
    if let Some(name) = builder {
        info!("${var} is set to {name:?}");
        eprintln!("${var} is set to {name:?}");

        if !name.is_empty() {
            if let Some(val) = buildkit_host {
                bail!("Overriding ${BUILDKIT_HOST}={val:?} while setting ${var}={name:?} is unsupported")
            }
        }
    }

    //CARGOGREEN_REMOTES ~= CCSV: host=URL;URL
    // URL;0:host=URL,ca,cert,..;1:URL,ca=,..;2:..  https://docs.docker.com/build/ci/github-actions/configure-builder/#append-additional-nodes-to-the-builder
    //=> colon CSV
    //=> keys= host,ca,cert,key,skip-tls-verify + name,description,from (enforce!)
    //=> when only URL given: craft name
    //error if creating fails || creating existing name but different values
    //error if given builder does not have exactly these remotes (ESC name,description,from)
    //error if any of these is also set: DOCKER_HOST, DOCKER_CONTEXT, BUILDKIT_HOST
    //docker context create --help
    //
    // docker context create amd64 --docker host=ssh://root@x.x.x.220
    // docker context create arm64 --docker host=ssh://root@x.x.x.72
    // docker buildx create --name multiarch-builder amd64 [--platform linux/amd64]
    // docker buildx create --name multiarch-builder --append arm64 [--platform linux/arm64]
    // docker buildx build --builder multiarch-builder -t dustinrue/buildx-example --platform linux/amd64,linux/arm64,linux/arm/v6 .
    // https://dustinrue.com/2021/12/using-a-remote-docker-engine-with-buildx/
    //
    // https://github.com/moby/buildkit/issues/4268#issuecomment-2128464135
    // docker buildx create --name amd64-builder --driver docker-container --platform linux/amd64 ssh://user@remote-machine
    // docker buildx build --builder amd64-builder --load .
    //
    //docker use builder on other host
    //https://dev.to/aboozar/build-docker-multi-platform-image-using-buildx-remote-builder-node-5631

    // Then the builder: needed by cmd calls
    var = ENV_BUILDER_IMAGE!();
    if green.builder.image.is_some() {
        bail!("${var} can only be set through the environment variable")
    }
    if let Ok(builder_image) = env::var(var) {
        let img = builder_image
            .as_str()
            .try_into()
            .map_err(|e| anyhow!("${var}={builder_image:?} {e}"))?;
        // Don't use 'maybe_lock_image', only 'fetch_digest': cmd uses builder.
        green.builder.image = Some(fetch_digest(&img).await?);
    }

    green.maybe_setup_builder(builder.cloned()).await?;

    var = ENV_SYNTAX_IMAGE!();
    if !green.syntax.is_empty() {
        bail!("${var} can only be set through the environment variable")
    }
    if let Ok(syntax) = env::var(var) {
        green.syntax = syntax.as_str().try_into().map_err(|e| anyhow!("${var}={syntax:?} {e}"))?;
    }
    // Use local hashed image if one matching exists locally
    green.syntax = green.maybe_lock_image(&green.syntax).await?;
    // otherwise default to a hash found through some Web API
    green.syntax = fetch_digest(&green.syntax).await?;
    if !green.syntax.stable_syntax_frontend() {
        // Enforce a known stable syntax + allow pinning to digest
        bail!("${var} must be a digest of {}", SYNTAX_IMAGE.as_str())
    }

    var = ENV_FINAL_PATH!();
    if green.r#final.path.is_some() {
        bail!("${var} can only be set through the environment variable")
    }
    // TODO? provide a way to export final as flatpack
    if let Ok(path) = env::var(var) {
        if path.is_empty() {
            bail!("${var} is empty")
        }
        if path == "-" {
            bail!("${var} must not be {path:?}")
        }
        let path = camino::absolute_utf8(path)
            .map_err(|e| anyhow!("Failed canonicalizing ${var}: {e}"))?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| anyhow!("Failed `mkdir -p {dir}`: {e}"))?;
        }
        green.r#final.path = Some(path);
    }

    var = ENV_FINAL_PATH_NONPRIMARY!();
    if green.r#final.path_nonprimary {
        bail!("${var} can only be set through the environment variable")
    }
    if let Ok(v) = env::var(var) {
        if v.is_empty() {
            bail!("${var} is empty")
        }
        if v != "1" {
            bail!("${var} must only be '1'")
        }
        green.r#final.path_nonprimary = true;
    }

    if !green.base.image.locked() {
        let mut base = green.maybe_lock_image(&green.base.image).await?;
        base = fetch_digest(&base).await?;
        green.base = green.base.lock_base_to(base);
    }

    let (mut with_network, mut finalized_block) = green.base.as_block();
    if !green.add.is_empty() {
        (with_network, finalized_block) = green.add.as_block(&finalized_block);
    }
    green.base.with_network = with_network;
    green.base.image_inline = Some(finalized_block.trim().to_owned());

    assert!(!green.base.image.is_empty(), "BUG: base_image set to {SYNTAX_IMAGE:?}");

    var = ENV_WITH_NETWORK!();
    if let Ok(val) = env::var(var) {
        green.base.with_network = val.parse().map_err(|e| anyhow!("${var}={val:?} {e}"))?;
    }
    if let Ok(val) = env::var("CARGO_NET_OFFLINE") {
        if val == "1" {
            green.base.with_network = Network::None;
        }
    }

    // TODO? docker dial-stdio proxy
    // https://github.com/docker/cli/blob/9bb1a62735174e9220d84fecc056a0ef8a1fc26f/cli/command/system/dial_stdio.go

    // https://docs.docker.com/engine/context/working-with-contexts/
    // https://docs.docker.com/engine/security/protect-access/

    // FIXME "multiplex conns to daemon" https://github.com/docker/buildx/issues/2564#issuecomment-2207435201
    // > If you do have docker context created already on ssh endpoint then you don't need to set the ssh address again on buildx create, you can use the context name or let it use the active context.

    // https://linuxhandbook.com/docker-remote-access/
    // https://thenewstack.io/connect-to-remote-docker-machines-with-docker-context/
    // https://www.cyberciti.biz/faq/linux-unix-reuse-openssh-connection/
    // https://github.com/moby/buildkit/issues/4268#issuecomment-2128464135
    // https://github.com/moby/buildkit/blob/v0.15.1/session/sshforward/sshprovider/agentprovider.go#L119

    // https://crates.io/crates/async-ssh2-tokio
    // https://crates.io/crates/russh

    Ok(green)
}

pub(crate) async fn maybe_prebuild_base(green: &Green) -> Result<()> {
    let mut containerfile = green.new_containerfile();
    containerfile.pushln(green.base.image_inline.as_deref().unwrap());

    let fname = format!("{PKG}-{RST}-{}.Dockerfile", containerfile.hashed());
    let sentinel = tmp().join(format!("{fname}.done"));
    info!("checking the existence of {sentinel}");
    if sentinel.exists() {
        return Ok(());
    }

    let path = tmp().join(fname);
    containerfile.write_to(&path)?;

    // Turns out --network is part of BuildKit's cache key, so an initial online build
    // won't cache hit on later offline builds.
    green
        .build_cacheonly(&path, &RUST)
        .await
        .inspect(|_| {
            if let Err(e) = fs::write(&sentinel, "") {
                warn!("Failed creating sentinel {sentinel}: {e}")
            }
        })
        .map_err(|e| {
            let containerfile = containerfile.remove_from(&path);
            anyhow!("{containerfile}\n\nUnable to build {RST}: {e}")
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

    let imgs: Vec<_> = [
        // NOTE: we don't pull ENV_CACHE_IMAGES
        (env::var(ENV_SYNTAX_IMAGE!()).ok(), Some(&green.syntax)),
        (env::var(ENV_BASE_IMAGE!()).ok(), Some(&green.base.image)),
        (env::var(ENV_BUILDER_IMAGE!()).ok(), green.builder.image.as_ref()),
    ]
    .into_iter()
    .filter_map(|(user_input, img)| img.map(|img| (user_input, img)))
    .map(|(user_input, img)| {
        if img.locked() && user_input.map(|x| !x.contains("@sha256:")).unwrap_or(true) {
            // Don't pull a locked image unless that's what's asked
            // Otherwise, pull unlocked
            img.unlocked()
        } else {
            img.to_owned()
        }
    })
    .collect();

    let mut containerfile = green.new_containerfile();

    let imger = |img: &str| img.replace(['/', ':'], "-");
    let ddb = green.builder.is_default();

    for img in imgs.iter().filter(|_| !ddb) {
        let img = img.noscheme();
        containerfile.push(&format!("FROM --platform=$BUILDPLATFORM {img} AS {}\n", imger(img)));
    }

    let stage = Stage::new("cargo-fetch").unwrap();
    let stager = |i| format!("{stage}-{i}");

    let mut leaves = 0;
    // 127: https://github.com/docker/docs/issues/8230
    for (i, pkgs) in packages.chunks(127).enumerate() {
        leaves = i;

        containerfile.push(&format!("FROM scratch AS {}\n", stager(i)));

        let (name, version, hash) = &pkgs[0];
        debug!("will fetch crate {name}: {version}");
        containerfile.pushln(cratesio::add_step(name, version, hash).trim());

        for (name, version, hash) in &pkgs[1..] {
            debug!("will fetch crate {name}: {version}");
            containerfile.pushln(cratesio::add_step(name, version, hash).trim());
        }
    }
    containerfile.push(&format!("FROM scratch AS {stage}\n"));
    for leaf in 0..=leaves {
        containerfile.push(&format!("COPY --from={} / /\n", stager(leaf)));
    }

    for img in imgs.iter().filter(|_| !ddb) {
        let imgd = imger(img.noscheme());
        containerfile.push(&format!("COPY --from={imgd} / /{imgd}\n"));
    }

    let fname = format!(
        "{PKG}-fetch-{}.Dockerfile",
        hash(&(containerfile.hashed() + &format!("{imgs:?}")))
    );
    let sentinel = tmp().join(format!("{fname}.done"));
    info!("checking the existence of {sentinel}");
    if sentinel.exists() {
        return Ok(());
    }

    let path = tmp().join(fname);
    containerfile.write_to(&path)?;

    let imgs_is_empty = imgs.is_empty();

    let load_to_docker = async {
        if imgs_is_empty || !ddb {
            return Ok(());
        }
        pull(&green, imgs).await // NOTE: can't pull these with build(..): they won't get --load'ed
    };

    let cache_packages = async {
        if packages.is_empty() && (imgs_is_empty || ddb) {
            return Ok(());
        }
        green.build_cacheonly(&path, &stage).await
    };

    let ((), ()) = try_join!(load_to_docker, cache_packages).inspect(|_| {
        if let Err(e) = fs::write(&sentinel, "") {
            warn!("Failed creating sentinel {sentinel}: {e}")
        }
    })?;
    Ok(())
}

async fn pull(green: &Green, imgs: Vec<ImageUri>) -> Result<()> {
    // TODO: nice TUI that handles concurrent progress
    iter(imgs.into_iter())
        .map(|img| async { do_pull(green, img).await })
        .buffer_unordered(10)
        .try_collect()
        .await
}

async fn do_pull(green: &Green, img: ImageUri) -> Result<()> {
    println!("Pulling {img}...");
    let mut cmd = green.cmd()?;
    cmd.arg("pull").arg(img.noscheme());
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
