use std::{
    env,
    fs::{self, OpenOptions},
    process::{Output, Stdio},
};

use anyhow::{anyhow, bail, Result};
use camino::Utf8PathBuf;
use log::{debug, info, trace};
use tokio::process::Command;

use crate::{
    base::BaseImage,
    cratesio::{self},
    envs::{builder_image, cache_image, incremental, internal, maybe_log, runner, DEFAULT_SYNTAX},
    extensions::ShowCmd,
    green::{Green, ENV_FINAL_PATH},
    lockfile::{find_lockfile, locked_crates},
    logging,
    runner::{build, fetch_digest, maybe_lock_image, runner_cmd},
    stage::Stage,
    PKG, REPO, VSN,
};

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

    // Not calling envs::{syntax,base_image} directly so value isn't locked now.
    // Goal: produce only fully-locked Dockerfiles/TOMLs

    assert!(env::var_os("CARGOGREEN").is_none());
    env::set_var("CARGOGREEN", "1");

    // Use local hashed image if one matching exists locally
    let syntax = maybe_lock_image(internal::syntax().unwrap_or(DEFAULT_SYNTAX.to_owned())).await;
    // otherwise default to a hash found through some Web API
    let syntax = fetch_digest(&syntax).await?;
    env::set_var(internal::RUSTCBUILDX_SYNTAX, syntax);
    //TODO: no longer allow completely changing syntax=
    //actually just allow setting digest part => enforce prefix up to before '@'
    //TODO: also start a tokio race between local and remote syntax digest lookups
    // start very early in cargo-green, pick winner here.

    let green = Green::new_from_env_then_manifest()?;

    if let Some(ref path) = green.final_path {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| anyhow!("Failed `mkdir -p {dir}`: {e}"))?;
        }
        cmd.env(ENV_FINAL_PATH, path);
    }

    let mut base_image = if let Some(ref base_image) = green.base_image {
        BaseImage::Image(base_image.clone())
    } else {
        BaseImage::from_rustc_v()?.maybe_lock_base().await
    };
    let base = base_image.base();
    if !base.contains('@') {
        base_image = base_image.lock_base_to(fetch_digest(&base).await?);
    }
    env::set_var(internal::RUSTCBUILDX_BASE_IMAGE, base_image.base());

    let (mut base_image_block, mut with_network) =
        if let Some(ref base_image_inline) = green.base_image_inline {
            (base_image_inline.clone(), true)
        } else {
            base_image.block()
        };
    if !green.add.apt_get.is_empty() || !green.add.apt.is_empty() || !green.add.apk.is_empty() {
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
            xx = "tonistiigi/xx", //lock dis
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

    let builder_image = builder_image().await;

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
    cmd.env(internal::RUSTCBUILDX_RUNNER, runner());

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
        // Images were pulled, we have to re-read their now-locked values now
        let builder_image = maybe_lock_image(builder_image.to_owned()).await;
        setup_build_driver("supergreen", builder_image.trim_start_matches("docker-image://"))
            .await?; // FIXME? maybe_..
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
async fn setup_build_driver(name: &str, builder_image: &str) -> Result<()> {
    if false {
        // TODO: reuse old state but try auto-upgrading builder impl
        try_removing_previous_builder(name).await;
    }

    let mut cmd = runner_cmd();
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

async fn try_removing_previous_builder(name: &str) {
    let mut cmd = runner_cmd();
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

pub(crate) async fn fetch(syntax: &str) -> Result<()> {
    let syntax = syntax.trim_start_matches("docker-image://");

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

    return build(runner(), &dockerfile_path, stage, &[].into(), None).await;
}
