use std::{
    env,
    fs::{self},
    sync::LazyLock,
};

use anyhow::{anyhow, bail, Result};
use futures::{stream::iter, StreamExt, TryStreamExt};
use log::{debug, info, warn};
use serde::Deserialize;
use tokio::try_join;
use version_compare::Version;

use crate::{
    base_image::{ENV_BASE_IMAGE, ENV_WITH_NETWORK},
    cratesio::{self},
    ext::CommandExt,
    green::Green,
    hash,
    image_uri::{ImageUri, BUILDKIT, SYNTAX},
    lockfile::{find_lockfile, locked_crates},
    logging::{self, maybe_log},
    pwd,
    runner::{
        self, build_cacheonly, fetch_digest, Network, Runner, BUILDKIT_HOST, BUILDX_BUILDER,
        DOCKER_BUILDKIT, DOCKER_CONTEXT, DOCKER_HOST,
    },
    stage::{Stage, RST, RUST},
    tmp, PKG, VSN,
};

// Env-only settings (no Cargo.toml equivalent setting)
pub(crate) const ENV_BUILDER_IMAGE: &str = "CARGOGREEN_BUILDER_IMAGE";
pub(crate) const ENV_FINAL_PATH: &str = "CARGOGREEN_FINAL_PATH";
pub(crate) const ENV_FINAL_PATH_NONPRIMARY: &str = "CARGOGREEN_FINAL_PATH_NONPRIMARY";
pub(crate) const ENV_RUNNER: &str = "CARGOGREEN_RUNNER";
pub(crate) const ENV_SYNTAX: &str = "CARGOGREEN_SYNTAX";

pub(crate) async fn main() -> Result<Green> {
    let mut green = Green::new_from_env_then_manifest()?;

    // Setting runner first as it's needed by many calls
    if green.runner != Runner::default() {
        bail!("${ENV_RUNNER} can only be set through the environment variable")
    }
    if let Ok(val) = env::var(ENV_RUNNER) {
        green.runner = val.parse().map_err(|e| anyhow!("${ENV_RUNNER}={val:?} {e}"))?;
    }

    // Read runner's envs only once and disallow conf overrides
    if !green.runner_envs.is_empty() {
        bail!("'runner_envs' setting cannot be set")
    }
    green.runner_envs = runner::envs();

    let builders = green.list_builders().await?;

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

    if green.builder_name.is_some() {
        bail!("builder-name can only be set through the environment variable")
    }
    let builder = green.runner_envs.get(BUILDX_BUILDER);
    if let Some(name) = builder {
        info!("${BUILDX_BUILDER} is set to {name:?}");
        eprintln!("${BUILDX_BUILDER} is set to {name:?}");

        if !name.is_empty() {
            if let Some(val) = buildkit_host {
                bail!("Overriding ${BUILDKIT_HOST}={val:?} while setting ${BUILDX_BUILDER}={name:?} is unsupported")
            }
        }
    }

    //CARGOGREEN_REMOTES ~= CCSV: host=URL;URL
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

    // Then builder_image as it's needed by cmd calls
    if green.builder_image.is_some() {
        bail!("${ENV_BUILDER_IMAGE} can only be set through the environment variable")
    }
    if let Ok(builder_image) = env::var(ENV_BUILDER_IMAGE) {
        let img = builder_image
            .clone()
            .try_into()
            .map_err(|e| anyhow!("${ENV_BUILDER_IMAGE}={builder_image:?} {e}"))?;
        // Don't use 'maybe_lock_image', only 'fetch_digest': cmd uses builder.
        green.builder_image = Some(fetch_digest(&img).await?);
    }

    green.maybe_setup_builder(builder.cloned(), &builders).await?;

    if !green.syntax.is_empty() {
        bail!("${ENV_SYNTAX} can only be set through the environment variable")
    }
    if let Ok(syntax) = env::var(ENV_SYNTAX) {
        green.syntax =
            syntax.clone().try_into().map_err(|e| anyhow!("${ENV_SYNTAX}={syntax:?} {e}"))?;
    }
    // Use local hashed image if one matching exists locally
    green.syntax = green
        .maybe_lock_image(&green.syntax)
        .await
        .map_err(|e| anyhow!("Failed locking {}: {e}", green.syntax))?;
    // otherwise default to a hash found through some Web API
    green.syntax = fetch_digest(&green.syntax).await?;
    if !green.syntax.stable_syntax_frontend() {
        // Enforce a known stable syntax + allow pinning to digest
        bail!("${ENV_SYNTAX} must be a digest of {}", SYNTAX.as_str())
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
        let path = camino::absolute_utf8(path)
            .map_err(|e| anyhow!("Failed canonicalizing ${ENV_FINAL_PATH}: {e}"))?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| anyhow!("Failed `mkdir -p {dir}`: {e}"))?;
        }
        green.final_path = Some(path);
    }
    if green.final_path_nonprimary {
        bail!("${ENV_FINAL_PATH_NONPRIMARY} can only be set through the environment variable")
    }
    if let Ok(v) = env::var(ENV_FINAL_PATH_NONPRIMARY) {
        if v.is_empty() {
            bail!("${ENV_FINAL_PATH_NONPRIMARY} is empty")
        }
        if v != "1" {
            bail!("${ENV_FINAL_PATH_NONPRIMARY} must only be '1'")
        }
        green.final_path_nonprimary = true;
    }

    if !green.image.base_image.locked() {
        let mut base = green
            .maybe_lock_image(&green.image.base_image)
            .await
            .map_err(|e| anyhow!("Failed locking {}: {e}", green.image.base_image))?;
        base = fetch_digest(&base).await?;
        green.image = green.image.lock_base_to(base);
    }

    let (mut with_network, mut finalized_block) = green.image.as_block();
    if !green.add.is_empty() {
        (with_network, finalized_block) = green.add.as_block(&finalized_block);
    }
    green.image.with_network = with_network;
    green.image.base_image_inline = Some(finalized_block.trim().to_owned());

    assert!(!green.image.base_image.is_empty(), "BUG: base_image set to {SYNTAX:?}");

    if let Ok(val) = env::var(ENV_WITH_NETWORK) {
        green.image.with_network =
            val.parse().map_err(|e| anyhow!("${ENV_WITH_NETWORK}={val:?} {e}"))?;
    }
    if let Ok(val) = env::var("CARGO_NET_OFFLINE") {
        if val == "1" {
            green.image.with_network = Network::None;
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

impl Green {
    async fn maybe_setup_builder(
        &mut self,
        env: Option<String>,
        builders: &[BuildxBuilder],
    ) -> Result<()> {
        let (managed, name) = match env.as_deref() {
            None | Some("supergreen") => (true, "supergreen"),
            Some("") => {
                if let Some(ref img) = self.builder_image {
                    bail!("Not using a builder, however ${ENV_BUILDER_IMAGE}={img:?} is set")
                }
                return Ok(());
            }
            Some(name) => (false, name),
        };

        if let Some(existing) = find_builder(name, builders) {
            let mut recreate = false;

            //if builder exists and builder_image is set, but doesnt match existing, and env.is_some() (= not managed) => error "builderimage doesnt match builder's image" (note: digest matching)
            //else (if builder is unset = env.is_none()): re-create builder.
            if let Some(ref img) = self.builder_image {
                if !existing.uses_image(img) {
                    if !managed {
                        bail!("Existing ${BUILDX_BUILDER}={name:?} does not match ${ENV_BUILDER_IMAGE}={img:?}")
                    }
                    recreate = true;
                }
            }

            //if ~~default builder image, and~~ builder exists, and buildkit tags shows newer version, and env.is_none(): re-create builder
            //else (builder name is set): print warning + upgrade command (CLI)
            if !existing.uses_version_newer_or_equal_to(&LATEST_BUILDKIT) {
                if managed {
                    recreate = true;
                } else {
                    eprintln!(
                        "
Existing ${BUILDX_BUILDER}={name:?} runs a BuildKit version older than v{latest}
Maybe try to remove and re-create your builder with:
    docker buildx rm {name} --keep-state
then run your cargo command again.
",
                        latest = LATEST_BUILDKIT.as_str(),
                    );
                }
            }

            if recreate {
                // First try keeping state...
                if self.try_removing_builder(name, true).await.is_err() {
                    // ...then stop messing about...
                    self.try_removing_builder(name, false).await?;
                }
                // ...and create afresh.
                self.create_builder(name).await?;
            }
        } else if !managed {
            bail!("${BUILDX_BUILDER}={name} does not exist")
        } else {
            self.create_builder(name).await?;
        }

        self.builder_maxready = find_builder(name, builders).is_some_and(|b| b.driver != "docker"); // Hopes for BUILDER_DRIVER
        self.builder_name = Some(name.to_owned());
        Ok(())
    }

    async fn create_builder(&self, name: &str) -> Result<()> {
        let mut cmd = self.cmd();
        cmd.args(["buildx", "create", "--bootstrap"])
            .args(["--name", name])
            .args(["--driver", BUILDER_DRIVER]);

        let img = if let Some(ref builder_image) = self.builder_image {
            builder_image.clone()
        } else {
            fetch_digest(&BUILDKIT).await?
        };
        cmd.arg(format!("--driver-opt=image={}", img.noscheme()));

        let (succeeded, _, stderr) = cmd.exec().await?;
        if !succeeded {
            let stderr = String::from_utf8_lossy(&stderr);
            bail!("BUG: failed to create builder: {stderr}")
        }
        Ok(())
    }

    async fn try_removing_builder(&self, name: &str, keep_state: bool) -> Result<()> {
        let mut cmd = self.cmd();
        cmd.args(["buildx", "rm", "--builder", name]);
        if keep_state {
            cmd.arg("--keep-state");
        } else {
            cmd.arg("--force");
        }

        let (succeeded, _, stderr) = cmd.exec().await?;
        if !succeeded {
            let stderr = String::from_utf8_lossy(&stderr);
            bail!("Failed to remove builder {name}: {stderr}")
        }
        Ok(())
    }

    async fn list_builders(&self) -> Result<Vec<BuildxBuilder>> {
        let mut cmd = self.cmd();
        cmd.args(["buildx", "ls", "--format=json"]);
        let (succeeded, stdout, stderr) = cmd.exec().await?;
        let stdout = String::from_utf8_lossy(&stdout);
        if !succeeded {
            let stderr = String::from_utf8_lossy(&stderr);
            // Stacking STDIOs as I have no clue how this can fail
            bail!("Failed listing builders: {stderr}{stdout}")
        }
        parse_builders(&stdout)
    }
}

#[inline]
fn parse_builders(json: &str) -> Result<Vec<BuildxBuilder>> {
    json.lines()
        .map(|line| serde_json::from_str::<BuildxBuilder>(line).map_err(Into::into))
        .collect::<Result<Vec<_>>>()
        .map_err(|e| anyhow!("Failed to decode builders list: {e}\n{json}"))
}

#[inline]
#[must_use]
fn find_builder<'a>(name: &str, builders: &'a [BuildxBuilder]) -> Option<&'a BuildxBuilder> {
    builders.iter().find(|b| b.name == name)
}

#[test]
fn find_builders() {
    let json_bla = r#"
{"Current":false,"Driver":"docker-container","Dynamic":false,"LastActivity":"2025-08-09T11:39:54Z","Name":"bla","Nodes":[{"DriverOpts":{"image":"docker.io/moby/buildkit:buildx-stable-1"},"Endpoint":"unix:///var/run/docker.sock","Flags":["--allow-insecure-entitlement=network.host"],"GCPolicy":[{"all":false,"filter":["type==source.local,type==exec.cachemount,type==source.git.checkout"],"keepDuration":172800000000000,"maxUsedSpace":512000000,"minFreeSpace":0,"reservedSpace":0},{"all":false,"filter":null,"keepDuration":5184000000000000,"maxUsedSpace":100000000000,"minFreeSpace":94000000000,"reservedSpace":10000000000},{"all":false,"filter":null,"keepDuration":0,"maxUsedSpace":100000000000,"minFreeSpace":94000000000,"reservedSpace":10000000000},{"all":true,"filter":null,"keepDuration":0,"maxUsedSpace":100000000000,"minFreeSpace":94000000000,"reservedSpace":10000000000}],"IDs":["zh05kd8qdrkor9k2h15br199l"],"Labels":{"org.mobyproject.buildkit.worker.executor":"oci","org.mobyproject.buildkit.worker.hostname":"3cc514a6ea5c","org.mobyproject.buildkit.worker.network":"host","org.mobyproject.buildkit.worker.oci.process-mode":"sandbox","org.mobyproject.buildkit.worker.selinux.enabled":"false","org.mobyproject.buildkit.worker.snapshotter":"overlayfs"},"Name":"bla0","Platforms":["linux/amd64","linux/amd64/v2","linux/amd64/v3","linux/amd64/v4","linux/386"],"Status":"running","Version":"v0.22.0"}]}
    "#;
    let builders_bla = parse_builders(json_bla.trim()).unwrap();
    assert_eq!(find_builder("beepboop", &builders_bla), None);
    assert_eq!(
        find_builder("bla", &builders_bla).unwrap(),
        &BuildxBuilder {
            name: "bla".to_owned(),
            driver: BUILDER_DRIVER.to_owned(),
            nodes: vec![BuilderNode {
                version: Some("v0.22.0".to_owned()),
                driver_opts: Some(DriverOpts {
                    image: Some("docker.io/moby/buildkit:buildx-stable-1".to_owned()),
                }),
            }],
        }
    );

    let json_default = r#"
{"Current":true,"Driver":"docker","Dynamic":false,"LastActivity":"2025-08-11T13:30:33Z","Name":"default","Nodes":[{"Endpoint":"default","GCPolicy":[{"all":false,"filter":["type==source.local,type==exec.cachemount,type==source.git.checkout"],"keepDuration":172800000000000,"maxUsedSpace":6494262707,"minFreeSpace":0,"reservedSpace":0},{"all":false,"filter":null,"keepDuration":5184000000000000,"maxUsedSpace":6000000000,"minFreeSpace":24000000000,"reservedSpace":47000000000},{"all":false,"filter":null,"keepDuration":0,"maxUsedSpace":6000000000,"minFreeSpace":24000000000,"reservedSpace":47000000000},{"all":true,"filter":null,"keepDuration":0,"maxUsedSpace":6000000000,"minFreeSpace":24000000000,"reservedSpace":47000000000}],"IDs":["4ff1ee7f-a3ff-4df0-ad6e-9d0162ddbda5"],"Labels":{"org.mobyproject.buildkit.worker.moby.host-gateway-ip":"172.17.0.1"},"Name":"default","Platforms":["linux/amd64","linux/amd64/v2","linux/amd64/v3","linux/amd64/v4","linux/386"],"Status":"running","Version":"v0.23.2"}]}
    "#;
    let builders_default = parse_builders(json_default.trim()).unwrap();
    assert_eq!(
        find_builder("default", &builders_default).unwrap(),
        &BuildxBuilder {
            name: "default".to_owned(),
            driver: "docker".to_owned(),
            nodes: vec![BuilderNode { version: Some("v0.23.2".to_owned()), driver_opts: None }],
        }
    );
}

/// https://docs.docker.com/build/builders/drivers/docker-container/#qemu
/// https://docs.docker.com/build/cache/backends/
const BUILDER_DRIVER: &str = "docker-container";

#[derive(Deserialize)]
#[cfg_attr(test, derive(Debug, PartialEq))]
#[serde(rename_all = "PascalCase")]
struct BuilderNode {
    driver_opts: Option<DriverOpts>,
    version: Option<String>,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(Debug, PartialEq))]
struct DriverOpts {
    /// An ImageUri without ^docker-image://
    image: Option<String>,
}

/// https://docs.docker.com/build/builders/drivers/
#[derive(Deserialize)]
#[cfg_attr(test, derive(Debug, PartialEq))]
#[serde(rename_all = "PascalCase")]
struct BuildxBuilder {
    name: String,
    /// Not an enum: future-proof (docker, docker-container, ..)
    driver: String,
    nodes: Vec<BuilderNode>,
}

impl BuildxBuilder {
    fn uses_image(&self, img: &ImageUri) -> bool {
        self.nodes.iter().any(|BuilderNode { driver_opts, .. }| {
            driver_opts.iter().any(|DriverOpts { image }| {
                if image.as_ref().is_some_and(|i| i.contains('@')) {
                    image.as_deref() == Some(img.noscheme())
                } else {
                    image.as_deref() == Some(img.unlocked().noscheme())
                }
            })
        })
    }

    fn uses_version_newer_or_equal_to(&self, latest: &Version) -> bool {
        self.nodes.iter().any(|BuilderNode { version, .. }| {
            version.as_deref().is_some_and(|v| {
                let v = v.trim_start_matches('v');
                Version::from(v).is_some_and(|ref v| v >= latest)
            })
        })
    }
}

/// Not a Release Candidate
/// https://github.com/moby/buildkit/tags
static LATEST_BUILDKIT: LazyLock<Version> =
    LazyLock::new(|| Version::from(include_str!("latest_buildkit.txt").trim()).unwrap());

#[test]
fn uses_version_newer_or_equal_to() {
    assert!(Version::from("2").is_some_and(|ref v| v >= &LATEST_BUILDKIT));
}

pub(crate) async fn maybe_prebuild_base(green: &Green) -> Result<()> {
    let mut containerfile = green.new_containerfile();
    containerfile.pushln(green.image.base_image_inline.as_deref().unwrap());

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
    build_cacheonly(green, &path, RUST.clone())
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
        (env::var(ENV_SYNTAX).ok(), Some(&green.syntax)),
        (env::var(ENV_BASE_IMAGE).ok(), Some(&green.image.base_image)),
        (env::var(ENV_BUILDER_IMAGE).ok(), green.builder_image.as_ref()),
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
    let ddb = !green.builder_maxready; // using !maxready as a proxy for "using default docker builder"

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
        containerfile.pushln(&cratesio::add_step(name, version, hash));

        for (name, version, hash) in &pkgs[1..] {
            debug!("will fetch crate {name}: {version}");
            containerfile.pushln(&cratesio::add_step(name, version, hash));
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
        build_cacheonly(&green, &path, stage).await
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
    let mut cmd = green.cmd();
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
