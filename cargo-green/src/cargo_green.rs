use std::{
    env,
    fs::{self, OpenOptions},
    process::{Output, Stdio},
};

use anyhow::{anyhow, bail, Result};
use camino::{absolute_utf8, Utf8PathBuf};
use log::{debug, info, trace};
use tokio::process::Command;

use crate::{
    cratesio::{self},
    envs::{cache_image, incremental, internal, maybe_log},
    extensions::ShowCmd,
    green::{Green, ENV_BASE_IMAGE},
    lockfile::{find_lockfile, locked_crates},
    logging,
    runner::{build, fetch_digest, maybe_lock_image, runner_cmd},
    stage::Stage,
    PKG, REPO, VSN,
};

// Env-only settings (no Cargo.toml equivalent setting)
pub(crate) const ENV_BUILDER_IMAGE: &str = "CARGOGREEN_BUILDER_IMAGE";
pub(crate) const ENV_FINAL_PATH: &str = "CARGOGREEN_FINAL_PATH";
pub(crate) const ENV_RUNNER: &str = "CARGOGREEN_RUNNER";
pub(crate) const ENV_SYNTAX: &str = "CARGOGREEN_SYNTAX";

pub(crate) async fn main(cmd: &mut Command) -> Result<Green> {
    // TODO: TUI above cargo output (? https://docs.rs/prodash )

    if let Ok(log) = env::var("CARGOGREEN_LOG") {
        let mut val = String::new();
        for (var, def) in [
            (internal::RUSTCBUILDX_LOG, log),
            (internal::RUSTCBUILDX_LOG_PATH, "/tmp/cargo-green.log".to_owned()), // last
        ] {
            val = env::var(var).unwrap_or(def.to_owned());
            cmd.env(var, &val);
            env::set_var(var, &val);
        }
        let _ = OpenOptions::new().create(true).truncate(false).append(true).open(val);
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
    env::set_var(ENV_SYNTAX, &green.syntax);
    if !green.syntax.starts_with(SYNTAX) {
        // Enforce a known stable syntax + allow pinning to digest
        bail!("${ENV_SYNTAX} must be a digest of {SYNTAX}")
    }

    const BUILDER_IMAGE: &str = "docker-image://docker.io/moby/buildkit:buildx-stable-1";
    if !green.builder_image.is_empty() {
        bail!("${ENV_BUILDER_IMAGE} can only be set through the environment variable")
    }
    green.builder_image = env::var(ENV_BUILDER_IMAGE).unwrap_or_else(|_| BUILDER_IMAGE.to_owned());
    green.builder_image = maybe_lock_image(&green, &green.builder_image).await;
    // FIXME: should be Option<_> and default to local builder
    if !green.builder_image.starts_with("docker-image://") {
        bail!("${ENV_BUILDER_IMAGE} must be a docker-image://")
    }

    if green.final_path.is_some() {
        bail!("${ENV_FINAL_PATH} can only be set through the environment variable")
    }
    // NOTE: only settable via env
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
        let base = fetch_digest(&green.image.base_image).await?;
        green.image = green.image.lock_base_to(base);
    }
    env::set_var(ENV_BASE_IMAGE, &green.image.base_image); // TODO? drop

    let (mut base_image_block, mut with_network) = green.image.block();
    if !green.add.is_empty() {
        with_network = true;
        base_image_block = format!(
            r#"
FROM --platform=$BUILDPLATFORM {xx} AS xx
{base_image_block}
ARG TARGETPLATFORM
RUN \
  --mount=from=xx,source=/usr/bin/xx-apk,target=/usr/bin/xx-apk \
  --mount=from=xx,source=/usr/bin/xx-apt,target=/usr/bin/xx-apt \
  --mount=from=xx,source=/usr/bin/xx-apt,target=/usr/bin/xx-apt-get \
  --mount=from=xx,source=/usr/bin/xx-cc,target=/usr/bin/xx-c++ \
  --mount=from=xx,source=/usr/bin/xx-cargo,target=/usr/bin/xx-cargo \
  --mount=from=xx,source=/usr/bin/xx-cc,target=/usr/bin/xx-cc \
  --mount=from=xx,source=/usr/bin/xx-cc,target=/usr/bin/xx-clang \
  --mount=from=xx,source=/usr/bin/xx-cc,target=/usr/bin/xx-clang++ \
  --mount=from=xx,source=/usr/bin/xx-go,target=/usr/bin/xx-go \
  --mount=from=xx,source=/usr/bin/xx-info,target=/usr/bin/xx-info \
  --mount=from=xx,source=/usr/bin/xx-ld-shas,target=/usr/bin/xx-ld-shas \
  --mount=from=xx,source=/usr/bin/xx-verify,target=/usr/bin/xx-verify \
  --mount=from=xx,source=/usr/bin/xx-windres,target=/usr/bin/xx-windres \
    set -ex \
  && if command -v apk >/dev/null 2>&1; then \
       xx-apk add --no-cache {apk}; \
     elif command -v apt 2&>1; then \
      xx-apt install --no-install-recommends -y {apt}; \
     else \
      xx-apt-get install --no-install-recommends -y {apt_get}; \
     fi
"#,
            xx = "tonistiigi/xx", //TODO: lock dis
            apk = &green.add.apk.join(" "),
            apt = &green.add.apt.join(" "),
            apt_get = &green.add.apt_get.join(" "),
        )[1..]
            .to_owned();
    }
    //todo: pre build base image here + no lifting io + fail with nice error display
    env::set_var("RUSTCBUILDX_BASE_IMAGE_BLOCK_", base_image_block);
    cmd.env(
        internal::RUSTCBUILDX_RUNS_ON_NETWORK,
        internal::runs_on_network()
            .unwrap_or_else(|| if with_network { "default" } else { "none" }.to_owned()),
    );
    //TODO: CARGOGREEN_NETWORK= <unset> | default | none | host
    //=> see also `base-image-inline`
    //=> auto set to "default" when using `add.{apk,apt,apt-get}` and maybe others

    // don't pull
    if let Some(val) = cache_image() {
        cmd.env(internal::RUSTCBUILDX_CACHE_IMAGE, val);
        //TODO: $CARGOGREEN_IMAGE_CACHES? (comma separated)
    }

    if incremental() {
        cmd.env(internal::RUSTCBUILDX_INCREMENTAL, "1");
    }
    // RUSTCBUILDX_LOG
    // RUSTCBUILDX_LOG_PATH
    // RUSTCBUILDX_LOG_STYLE
    cmd.env(ENV_RUNNER, &green.runner);

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

    let builder_image = green.builder_image.trim_start_matches("docker-image://");

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

pub(crate) async fn fetch(green: Green) -> Result<()> {
    let syntax = green.syntax.trim_start_matches("docker-image://");

    logging::setup("fetch", internal::RUSTCBUILDX_LOG, internal::RUSTCBUILDX_LOG_STYLE);
    let _ = maybe_log();
    info!("{PKG}@{VSN} original args: {:?} pwd={:?}", env::args(), env::current_dir());

    let manifest_path_lockfile = find_lockfile().await?;
    debug!("using lockfile at {manifest_path_lockfile}");

    let packages = locked_crates(&manifest_path_lockfile).await?;
    info!("found {} packages", packages.len());
    if packages.is_empty() {
        return Ok(());
    }

    let stage = Stage::try_new("cargo-fetch")?;
    let stager = |i| format!("{stage}-{i}");

    let mut dockerfile = format!("# syntax={syntax}\n");
    dockerfile.push_str("# check=error=true\n");
    dockerfile.push_str(&format!("# Generated by {REPO} version {VSN}\n"));
    let mut leaves = 0;
    for (i, pkgs) in packages.chunks(127).enumerate() {
        leaves = i;

        dockerfile.push_str(&format!("FROM scratch AS {}\n", stager(i)));

        let (name, version, hash) = &pkgs[0];
        debug!("will fetch crate {name}: {version}");
        dockerfile.push_str(&cratesio::add_step(name, version, hash));

        for (name, version, hash) in &pkgs[1..] {
            debug!("will fetch crate {name}: {version}");
            dockerfile.push_str(&cratesio::add_step(name, version, hash));
        }
    }
    dockerfile.push_str(&format!("FROM scratch AS {stage}\n"));
    for leaf in 0..=leaves {
        dockerfile.push_str(&format!("COPY --from={} / /\n", stager(leaf)));
    }

    let cfetch: Utf8PathBuf = env::temp_dir().join("cargo-fetch").try_into()?;
    fs::create_dir_all(&cfetch).map_err(|e| anyhow!("Failed `mkdir -p {cfetch:?}`: {e}"))?;

    let dockerfile_path = cfetch.join("Dockerfile");
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

    //TODO: + concurrently run with pull_images(&green)  +  drop supergreen pull
    return build(&green, &dockerfile_path, stage, &[].into(), None).await;
}
