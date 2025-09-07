use std::{str::FromStr, sync::LazyLock};

use anyhow::{anyhow, bail, Result};
use log::info;
use serde::{Deserialize, Serialize};
use version_compare::Version;

use crate::{
    cargo_green::ENV_BUILDER_IMAGE,
    ext::CommandExt,
    green::Green,
    image_uri::ImageUri,
    runner::{fetch_digest, BUILDX_BUILDER},
};

/// TODO: move to :rootless
static BUILDKIT_IMAGE: LazyLock<ImageUri> =
    LazyLock::new(|| ImageUri::try_new("docker-image://docker.io/moby/buildkit:latest").unwrap());

/// https://docs.docker.com/build/builders/drivers/docker-container/#qemu
/// https://docs.docker.com/build/cache/backends/
const BUILDER_DRIVER: &str = "docker-container";

/// Not a Release Candidate
/// https://github.com/moby/buildkit/tags
static LATEST_BUILDKIT: LazyLock<Version> =
    LazyLock::new(|| Version::from(include_str!("latest_buildkit.txt").trim()).unwrap());

#[test]
fn uses_version_newer_or_equal_to() {
    assert!(Version::from("2").is_some_and(|ref v| v >= &LATEST_BUILDKIT));
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum Driver {
    Docker,
    DockerContainer,
    Other(String),
}

impl FromStr for Driver {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "docker" => Self::Docker,
            BUILDER_DRIVER => Self::DockerContainer,
            _ => Self::Other(s.to_owned()),
        })
    }
}

#[derive(Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Builder {
    // Sets which BuildKit builder to use, through $BUILDX_BUILDER.
    //
    // See https://docs.docker.com/build/building/variables/#buildx_builder
    //
    // * Unset: creates & handles a builder named "supergreen". Upgrades it if too old, while trying to keep old cached data
    // * Set to "": skips using a builder
    // * Set to "supergreen": uses existing and just warns if too old
    // * Set: use that as builder, no questions asked
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,

    // Shows which driver the configured builder uses.
    //
    // See https://docs.docker.com/build/drivers/
    // Also: https://docs.docker.com/build/drivers/docker-container/
    // Also: https://docs.docker.com/build/drivers/remote/
    // Also: https://docs.docker.com/build/drivers/kubernetes/
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) driver: Option<Driver>,

    // Sets which BuildKit builder version to use.
    //
    // See https://docs.docker.com/build/builders/
    //
    // # Use by setting this environment variable (no Cargo.toml setting):
    // CARGOGREEN_BUILDER_IMAGE="docker-image://docker.io/moby/buildkit:latest"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) image: Option<ImageUri>,
}

impl Builder {
    pub(crate) fn is_default(&self) -> bool {
        self.driver.as_ref().is_none_or(|d| d == &Driver::Docker)
    }

    pub(crate) fn has_maxready(&self) -> bool {
        self.driver.as_ref().is_some_and(|d| d == &Driver::DockerContainer)
    }
}

impl Green {
    pub(crate) async fn maybe_setup_builder(&mut self, env: Option<String>) -> Result<()> {
        let (managed, name) = match env.as_deref() {
            None | Some("supergreen") => (true, "supergreen"),
            Some("") => {
                if let Some(ref img) = self.builder.image {
                    bail!("Not using a builder, however ${ENV_BUILDER_IMAGE}={img:?} is set")
                }
                return Ok(());
            }
            Some(name) => (false, name),
        };

        let builders = self.list_builders().await?;
        let builder = find_builder(name, &builders);
        info!("found builder {builder:?}");
        if let Some(existing) = builder {
            let mut recreate = false;

            if let Some(ref img) = self.builder.image {
                if !existing.uses_image(img) {
                    if !managed {
                        bail!("Existing ${BUILDX_BUILDER}={name:?} does not match ${ENV_BUILDER_IMAGE}={img:?}")
                    }
                    recreate = true;
                }
            }

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

        if self.builder.image.is_none() {
            // Only informational: only used through showing envs values
            self.builder.image = builder.and_then(BuildxBuilder::first_image);
        }

        self.builder.driver = builder.map(|b| b.driver.parse().expect("infaillible"));
        self.builder.name = Some(name.to_owned());
        Ok(())
    }

    async fn create_builder(&mut self, name: &str) -> Result<()> {
        let mut cmd = self.cmd();
        cmd.args(["buildx", "create", "--bootstrap"])
            .args(["--name", name])
            .args(["--driver", BUILDER_DRIVER]);

        let img = if let Some(ref img) = self.builder.image {
            img.clone()
        } else {
            fetch_digest(&BUILDKIT_IMAGE).await?
        };
        cmd.arg(format!("--driver-opt=image={}", img.noscheme()));

        let (succeeded, _, stderr) = cmd.exec().await?;
        if !succeeded {
            let stderr = String::from_utf8_lossy(&stderr);
            bail!("BUG: failed to create builder: {stderr}")
        }

        self.builder.image = Some(img);
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

#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(rename_all = "PascalCase")]
struct BuilderNode {
    driver_opts: Option<DriverOpts>,
    version: Option<String>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
struct DriverOpts {
    /// An ImageUri without ^docker-image://
    image: Option<String>,
}

/// https://docs.docker.com/build/builders/drivers/
#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
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

    fn first_image(&self) -> Option<ImageUri> {
        let mut imgs: Vec<_> = self
            .nodes
            .iter()
            .filter_map(|n| n.driver_opts.as_ref().map(|o| o.image.clone()))
            .flatten()
            .filter_map(|img| ImageUri::try_from(format!("docker-image://{img}")).ok())
            .collect();
        imgs.sort();
        imgs.dedup();
        imgs.first().cloned()
    }
}
