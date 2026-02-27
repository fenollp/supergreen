use std::{
    env,
    fs::{self},
};

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info, warn};

use crate::{
    build::fetch_digest,
    cratesio::{self},
    experiments::EXPERIMENTS,
    green::{validate_csv, Green},
    image_uri::SYNTAX_IMAGE,
    lockfile::{find_lockfile, locked_crates},
    logging::{self, maybe_log},
    network::Network,
    pwd,
    runner::{Runner, BUILDKIT_HOST, DOCKER_BUILDKIT, DOCKER_CONTEXT, DOCKER_HOST},
    stage::{Stage, RST},
    tmp, ENV_FINAL_PATH, ENV_RUNNER, ENV_SYNTAX_IMAGE, PKG, VSN,
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

    // Get $CARGO_HOME only once and disallow conf overrides
    if green.cargo_home != "" {
        bail!("'cargo_home' setting cannot be set")
    }
    green.cargo_home = cargo_home()?;
    green.maybe_arrange_cratesio_index()?;

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

    var = ENV_EXPERIMENT!();
    if !green.experiment.is_empty() {
        bail!("${var} can only be set through the environment variable")
    }
    validate_csv(&mut green.experiment, ENV_EXPERIMENT!())?;
    let nopes: Vec<_> =
        green.experiment.iter().filter(|ex| !EXPERIMENTS.contains(&ex.as_str())).collect();
    if !nopes.is_empty() {
        bail!("${var} contains unknown experiment names: {nopes:?}")
    }

    Ok(green)
}

impl Green {
    /// cargo green supergreen sync
    /// * no cargo.lock needed, but use deps if available
    /// * pull images as they are given (maybe locked)
    /// * TODO: push caches (tags?) ~ find last containerfile and rerun that build with cacheto
    pub(crate) async fn prebuild(&self, require_lockfile: bool) -> Result<()> {
        logging::setup("prebuild");
        let _ = maybe_log();
        info!("{PKG}@{VSN} original args: {:?} pwd={:?}", env::args(), pwd());

        let mut packages = vec![];
        if let Err(e) = (async {
            let manifest_path_lockfile = find_lockfile().await?;
            debug!("using lockfile at {manifest_path_lockfile}");

            packages = locked_crates(&manifest_path_lockfile).await?;
            info!("found {} packages", packages.len());

            Ok(())
        })
        .await
        {
            if require_lockfile {
                return Err(e);
            }
        }

        let stage = Stage::new("prebuild").unwrap();
        let mut containerfile = self.new_containerfile();

        containerfile.pushln(self.base.image_inline.as_deref().unwrap());

        let stager = |i| format!("{stage}-{i}");
        let mut leaves = 0;
        // 127: https://github.com/docker/docs/issues/8230
        for (i, pkgs) in packages.chunks(127).enumerate() {
            leaves = i;

            containerfile.push(&format!("FROM scratch AS {}\n", stager(i)));

            let (name, version, hash) = &pkgs[0];
            let name_dash_version = format!("{name}-{version}");
            debug!("will fetch crate {name_dash_version}");
            containerfile.pushln(cratesio::add_step(name, &name_dash_version, hash).trim());

            for (name, version, hash) in &pkgs[1..] {
                let name_dash_version = format!("{name}-{version}");
                debug!("will fetch crate {name_dash_version}");
                containerfile.pushln(cratesio::add_step(name, &name_dash_version, hash).trim());
            }
        }

        containerfile.push(&format!("FROM scratch AS {stage}\n"));
        if leaves > 0 {
            for leaf in 0..=leaves {
                containerfile
                    .push(&format!("COPY --link --from={stg} / /{stg}\n", stg = stager(leaf)));
            }
        }
        containerfile.push(&format!("COPY --link --from={RST} / /{RST}\n"));

        let fname = format!("{PKG}-{VSN}-prebuilt-{}.Dockerfile", containerfile.hashed());

        let sentinel = tmp().join(format!("{fname}.done"));
        info!("checking the existence of {sentinel}");
        if sentinel.exists() {
            return Ok(());
        }

        let path = tmp().join(fname);
        containerfile.write_to(&path)?;

        self.build_cacheonly(&path, &stage)
            .await
            .inspect(|()| {
                if let Err(e) = fs::write(&sentinel, "") {
                    warn!("Failed creating sentinel {sentinel}: {e}")
                }
            })
            .map_err(|e| anyhow!("{path}\n\nUnable to prebuild: {e}"))
    }
}

fn cargo_home() -> Result<Utf8PathBuf> {
    home::cargo_home()
        .map_err(|e| anyhow!("bad $CARGO_HOME or something: {e}"))?
        .try_into()
        .map_err(|e| anyhow!("corrupted $CARGO_HOME path: {e}"))
}

pub(crate) fn rewrite_cargo_home(cargo_home: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    path.to_string().replacen(cargo_home.as_str(), "$CARGO_HOME", 1).into()
}
