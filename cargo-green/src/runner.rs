use std::{
    collections::HashMap, env, ffi::OsStr, fmt, process::Stdio, str::FromStr, sync::OnceLock,
};

use anyhow::{anyhow, bail, Result};
use camino::Utf8PathBuf;
use log::info;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::green::Green;

#[macro_export]
macro_rules! ENV_RUNNER {
    () => {
        "CARGOGREEN_RUNNER"
    };
}

// Envs from BuildKit/Buildx/Docker/Podman that we read
const BUILDKIT_COLORS: &str = "BUILDKIT_COLORS";
pub(crate) const BUILDKIT_HOST: &str = "BUILDKIT_HOST";
const BUILDKIT_PROGRESS: &str = "BUILDKIT_PROGRESS";
const BUILDKIT_TTY_LOG_LINES: &str = "BUILDKIT_TTY_LOG_LINES";
const BUILDX_CPU_PROFILE: &str = "BUILDX_CPU_PROFILE";
const BUILDX_MEM_PROFILE: &str = "BUILDX_MEM_PROFILE";
pub(crate) const DOCKER_BUILDKIT: &str = "DOCKER_BUILDKIT";
pub(crate) const DOCKER_CONTEXT: &str = "DOCKER_CONTEXT";
const DOCKER_DEFAULT_PLATFORM: &str = "DOCKER_DEFAULT_PLATFORM";
const DOCKER_HIDE_LEGACY_COMMANDS: &str = "DOCKER_HIDE_LEGACY_COMMANDS";
pub(crate) const DOCKER_HOST: &str = "DOCKER_HOST";

impl Runner {
    /// Read envs used by runner, once.
    ///
    /// * <https://docs.docker.com/engine/reference/commandline/cli/#environment-variables>
    /// * <https://docs.docker.com/build/building/variables/#build-tool-configuration-variables>
    pub(crate) fn envs(&self) -> HashMap<String, String> {
        [
            BUILDKIT_COLORS,
            BUILDKIT_HOST,
            BUILDKIT_PROGRESS,
            BUILDKIT_TTY_LOG_LINES,
            "BUILDX_BAKE_GIT_AUTH_HEADER",
            "BUILDX_BAKE_GIT_AUTH_TOKEN",
            "BUILDX_BAKE_GIT_SSH",
            BUILDX_BUILDER!(),
            DOCKER_BUILDKIT,
            "BUILDX_CONFIG",
            BUILDX_CPU_PROFILE,
            "BUILDX_EXPERIMENTAL",
            "BUILDX_GIT_CHECK_DIRTY",
            "BUILDX_GIT_INFO",
            "BUILDX_GIT_LABELS",
            BUILDX_MEM_PROFILE,
            "BUILDX_METADATA_PROVENANCE",
            "BUILDX_METADATA_WARNINGS",
            "BUILDX_NO_DEFAULT_ATTESTATIONS",
            "BUILDX_NO_DEFAULT_LOAD",
            "DOCKER_API_VERSION",
            "DOCKER_CERT_PATH",
            "DOCKER_CONFIG",
            "DOCKER_CONTENT_TRUST",
            "DOCKER_CONTENT_TRUST_SERVER",
            DOCKER_CONTEXT,
            DOCKER_DEFAULT_PLATFORM,
            DOCKER_HIDE_LEGACY_COMMANDS,
            DOCKER_HOST,
            "DOCKER_TLS",
            "DOCKER_TLS_VERIFY",
            "EXPERIMENTAL_BUILDKIT_SOURCE_POLICY",
            "HTTP_PROXY",  //TODO: hinders reproducibility
            "HTTPS_PROXY", //TODO: hinders reproducibility
            "NO_PROXY",    //TODO: hinders reproducibility
        ]
        .into_iter()
        .filter_map(|k| env::var(k).ok().map(|v| (k.to_owned(), v)))
        .collect()
    }

    /// Strip out envs that don't affect a build's outputs:
    pub(crate) fn buildnoop_envs(&self) -> Vec<&OsStr> {
        if *self == Self::Docker {
            [
                BUILDKIT_COLORS,
                BUILDKIT_HOST,
                BUILDKIT_PROGRESS,
                BUILDKIT_TTY_LOG_LINES,
                BUILDX_BUILDER!(),
                BUILDX_CPU_PROFILE,
                BUILDX_MEM_PROFILE,
                DOCKER_CONTEXT,
                DOCKER_DEFAULT_PLATFORM,
                DOCKER_HIDE_LEGACY_COMMANDS,
                DOCKER_HOST,
            ]
            .into_iter()
            .map(OsStr::new)
            .collect()
        } else {
            vec![]
        }
    }

    /// Strip out flags that don't affect a build's outputs:
    pub(crate) fn buildnoop_flags(&self) -> impl Iterator<Item = &str> {
        ["--cache-from=", "--cache-to=", "--no-cache"].into_iter()
    }
}

impl Green {
    pub(crate) fn cmd(&self) -> Result<Command> {
        let mut cmd = Command::new(self.runner.executable()?); //FIXME persist to Green
        cmd.kill_on_drop(true); // Underlying OS process dies with us
        cmd.stdin(Stdio::null());
        if false {
            cmd.arg("--debug");
        }
        cmd.env_clear(); // Pass all envs explicitly only
        cmd.env(DOCKER_BUILDKIT, "1"); // BuildKit is used by either runner

        if let Some(ref name) = self.builder.name {
            cmd.env(BUILDX_BUILDER!(), name);
        }

        for (var, val) in &self.runner_envs {
            if [BUILDX_BUILDER!(), DOCKER_BUILDKIT].contains(&var.as_str()) {
                continue;
            }
            info!("passing through runner setting: ${var}={val:?}");
            cmd.env(var, val);
        }

        Ok(cmd)
    }
}

#[derive(Debug, Copy, Clone, Default, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Runner {
    #[default]
    Docker,
    Podman,
    None,
}

/// Resolve to an executable binary.
///
/// Somehow, not finding but executing `docker` may give the following errors (on arm Darwin):
///
/// ```text
/// Error: Failed to spawn DOCKER_BUILDKIT="1" docker buildx ls --format=json: No such file or directory (os error 2)
/// ```
///
/// ```text
/// Calling DOCKER_BUILDKIT="1" /usr/local/bin/docker buildx create --bootstrap --name supergreen --driver docker-container --driver-opt=image=docker.io[...]
/// Error: BUG: failed to create builder: #1 [internal] booting buildkit
/// #1 pulling image docker.io/moby/buildkit:latest@sha256:faffcac91decfb3b981234bf2762d88ed6c90771b689a3d8a5049cd0e874759a done
/// #1 ERROR: error getting credentials - err: exec: "docker-credential-desktop": executable file not found in $PATH, out: ``
/// ------
///  > [internal] booting buildkit:
/// ------
/// ERROR: error getting credentials - err: exec: "docker-credential-desktop": executable file not found in $PATH, out: ``
/// ```
impl Runner {
    pub(crate) fn executable(&self) -> Result<&'static Utf8PathBuf> {
        static EXE: OnceLock<Utf8PathBuf> = OnceLock::new();
        if let Some(exe) = EXE.get() {
            return Ok(exe);
        }

        which::which(self.to_string())
            .map(|exe| exe.try_into().unwrap())
            .inspect(|exe| info!("Using runner {exe}"))
            .inspect(|exe| eprintln!("Using runner {exe}"))
            .map(|exe| {
                let _ = EXE.set(exe);
                EXE.get().unwrap()
            })
            .map_err(|e| anyhow!("No such {self} runner: {e}"))
    }
}

impl fmt::Display for Runner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Docker => write!(f, "docker"),
            Self::Podman => write!(f, "podman"),
            Self::None => write!(f, "none"),
        }
    }
}

impl FromStr for Runner {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "docker" => Ok(Self::Docker),
            "podman" => Ok(Self::Podman),
            "none" => Ok(Self::None),
            _ => {
                let all: Vec<_> = [Self::Docker, Self::Podman, Self::None]
                    .iter()
                    .map(ToString::to_string)
                    .collect();
                bail!("Runner must be one of {all:?}")
            }
        }
    }
}
