use anyhow::{Result, anyhow, bail};
use camino::Utf8PathBuf;
use log::{debug, info, warn};

use crate::{
    PKG, VSN,
    all_our_envs::{
        CARGOGREEN_BUILDER_IMAGE, CARGOGREEN_EXPERIMENT, CARGOGREEN_FINAL_PATH,
        CARGOGREEN_SYNTAX_IMAGE, CARGOGREEN_WITH_NETWORK, find_unknowns,
    },
    base_image::{BASE_IMAGE, BASE_IMAGE_LOCKED},
    cratesio::{self},
    dirs::{Dirs, cargo_home},
    experiments::EXPERIMENTS,
    r#final::Final,
    green::{Green, validate_csv},
    image_uri::{SYNTAX_IMAGE_LOCKED, fetch_digest},
    lockfile::{find_lockfile, locked_crates},
    network::Network,
    redact::Vars,
    remote::RemoteDaemon,
    runner::Runner,
    stage::{RST, Stage},
    sys::fs,
};

pub(crate) struct Startup {
    pub(crate) toolchain: String,
    pub(crate) is_install: bool,
    pub(crate) pwd: Utf8PathBuf,
    pub(crate) env: Vars,
    pub(crate) verbose: bool,
    pub(crate) runner: Runner,
    pub(crate) dirs: Option<Dirs>,
}

pub(crate) async fn main(
    Startup { toolchain, is_install, pwd, env, verbose, runner, dirs }: Startup,
    remote: Option<&RemoteDaemon>,
) -> Result<Green> {
    let mut green = Green::new_from_env_then_manifest(is_install, &pwd, env).await?;

    // Needed by every cmd call below
    green.verbose = verbose;

    // Fail early rather than once the build is done
    // TODO? provide a way to export final as flatpack
    if let Some(path) = green.env(CARGOGREEN_FINAL_PATH!()) {
        green.r#final.path = Some(Final::path_from_env(path, CARGOGREEN_FINAL_PATH)?);
    }

    green.runner = runner;

    green.paths.cargo_home = cargo_home()?;
    green.paths.setup()?;
    green.paths.cwd = pwd;
    green.paths.dirs = dirs;

    if let Some(builder_image) = green.env(CARGOGREEN_BUILDER_IMAGE!()) {
        let img = builder_image
            .try_into()
            .map_err(|e| anyhow!("{CARGOGREEN_BUILDER_IMAGE}={builder_image:?} {e}"))?;
        // Don't use 'maybe_lock_image', only 'fetch_digest': cmd uses builder.
        green.builder.image = Some(fetch_digest(&green.runner, &img).await?);
    }

    let builder = green.runner_env(BUILDX_BUILDER!()).map(ToOwned::to_owned);
    green.maybe_setup_builder(builder.as_deref(), remote).await?;
    green.maybe_inspect_builder().await?;

    if let Some(syntax) = green.env(CARGOGREEN_SYNTAX_IMAGE!()) {
        green.syntax =
            syntax.try_into().map_err(|e| anyhow!("{CARGOGREEN_SYNTAX_IMAGE}={syntax:?} {e}"))?;
    }
    if green.syntax.is_empty() {
        // TODO: dynamically lock, if network is up.
        green.syntax = SYNTAX_IMAGE_LOCKED.clone();
    }
    // Use local hashed image if one matching exists locally
    green.syntax = green.maybe_lock_image(&green.syntax).await?;
    // otherwise default to a hash found through some Web API
    green.syntax = fetch_digest(&green.runner, &green.syntax).await?;

    if !green.base.image.locked() {
        if green.base.image.as_str() == BASE_IMAGE.as_str() {
            // TODO: dynamically lock, if network is up.
            green.base.image = BASE_IMAGE_LOCKED.clone();
        }
        let base = green.maybe_lock_image(&green.base.image).await?;
        green.base.image = fetch_digest(&green.runner, &base).await?;
    }
    let target: Option<String> =
        pico_args::Arguments::from_env().opt_value_from_str("--target").ok().flatten();
    green.base =
        green.base.make_block(&toolchain, &green.components, target.as_deref(), &green.add)?;

    if let Some(val) = green.env(CARGOGREEN_WITH_NETWORK!()) {
        green.base.with_network =
            val.parse().map_err(|e| anyhow!("{CARGOGREEN_WITH_NETWORK}={val:?} {e}"))?;
    }
    if let Some(val) = green.env(CARGO_NET_OFFLINE!())
        && val == "1"
    {
        green.base.with_network = Network::None;
    }

    validate_csv(&mut green.experiment, CARGOGREEN_EXPERIMENT!(), &green.env)?;
    let nopes: Vec<_> =
        green.experiment.iter().filter(|ex| !EXPERIMENTS.contains(&ex.as_str())).collect();
    if !nopes.is_empty() {
        bail!("{CARGOGREEN_EXPERIMENT} contains unknown experiment names: {nopes:?}")
    }

    let unknowns = find_unknowns(&green.env);
    if !unknowns.is_empty() {
        bail!("Ignored environment variable(s): {}", unknowns.join(" "))
    }

    Ok(green)
}

impl Green {
    /// cargo green supergreen sync
    /// * no cargo.lock needed, but use deps if available
    /// * pull images as they are given (maybe locked)
    /// * TODO: push caches (tags?) ~ find last containerfile and rerun that build with cacheto
    pub(crate) async fn prebuild(&self, require_lockfile: bool, is_install: bool) -> Result<()> {
        info!("{PKG}@{VSN} original args: {:?} pwd={:?}", self.env, self.paths.cwd);

        let mut packages = vec![];
        if !is_install
            && let Err(e) = async {
                let manifest_path_lockfile = find_lockfile(&self.paths.cwd).await?;
                debug!("using lockfile at {manifest_path_lockfile}");

                packages = locked_crates(&manifest_path_lockfile).await?;
                info!("found {} packages", packages.len());

                Ok(())
            }
            .await
            // TODO: probably drop `require_lockfile`
            && require_lockfile
        {
            return Err(e);
        }

        let stage = Stage::new("prebuild").unwrap();
        let mut containerfile = self.new_containerfile();

        containerfile.pushln(&self.base.image_inline);

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

        let sentinel = self.sentinel_path(&format!("{stage}-{}", containerfile.hashed()), "done");
        info!("checking the existence of {sentinel}");
        if sentinel.exists() {
            return Ok(());
        }

        let path = sentinel.with_extension("Dockerfile");
        containerfile.write_to(&path)?;

        if self.runner.is_none() {
            info!("Skipping prebuild (runner:{})", self.runner);
            return Ok(());
        }
        self.build_cacheonly(&path, &stage)
            .await
            .inspect(|()| {
                if let Err(e) = fs().write(&sentinel, "") {
                    warn!("Failed creating sentinel {sentinel}: {e}")
                }
            })
            .map_err(|e| anyhow!("{path}\n\nUnable to prebuild: {e}"))
    }
}
