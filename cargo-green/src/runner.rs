use std::{collections::HashMap, env, ffi::OsStr, fmt, str::FromStr, sync::OnceLock};

use anyhow::{Result, anyhow, bail};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Copy, Clone, Default, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Runner {
    #[default]
    Docker,
    Podman,
    None,
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

impl Runner {
    #[must_use]
    pub(crate) fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    #[must_use]
    pub(crate) fn is_buildkit(&self) -> bool {
        matches!(self, Self::Docker | Self::Podman)
    }

    /// Resolve to an executable binary.
    pub(crate) fn executable(&self) -> Result<&'static Utf8PathBuf> {
        static EXE: OnceLock<Utf8PathBuf> = OnceLock::new();
        if let Some(exe) = EXE.get() {
            return Ok(exe);
        }

        let runner = self.to_string();
        let exe = which::which(&runner).map_err(|e| anyhow!("No such {self} runner: {e}"))?;
        let exe = exe.try_into().map_err(|e| anyhow!("Path to {runner} is not utf-8: {e}"))?;

        let _ = EXE.set(exe);
        Ok(EXE.get().unwrap())
    }

    /// Read envs used by runner, once.
    ///
    /// * <https://docs.docker.com/engine/reference/commandline/cli/#environment-variables>
    ///   * `BUILDKIT_PROGRESS`
    ///   * `DOCKER_*`
    ///   * `HTTP_PROXY`
    ///   * `HTTPS_PROXY`
    ///   * `NO_COLOR`
    ///   * `NO_PROXY`
    /// * <https://docs.docker.com/build/building/variables/#build-tool-configuration-variables>
    ///   * `BUILDKIT_*`
    ///   * `BUILDX_*`
    ///   * `EXPERIMENTAL_BUILDKIT_*`
    pub(crate) fn envs(&self) -> HashMap<String, String> {
        env::vars()
            .filter(|(k, _)| {
                [
                    "HTTP_PROXY",  //TODO: hinders reproducibility
                    "HTTPS_PROXY", //TODO: hinders reproducibility
                    "NO_PROXY",    //TODO: hinders reproducibility
                    PATH!(),       // Required at least on macOS
                    "NO_COLOR",
                ]
                .contains(&k.as_str())
                    || ["BUILDKIT_", "BUILDX_", "DOCKER_", "EXPERIMENTAL_BUILDKIT_"]
                        .iter()
                        .any(|prefix| k.starts_with(prefix))
            })
            .collect()
    }

    /// Strip out envs that don't affect a build's outputs:
    pub(crate) fn buildnoop_envs(&self) -> Vec<&OsStr> {
        let mut noops = vec![
            // Common regardless of Runner
            OsStr::new("NO_COLOR"),
            OsStr::new(PATH!()),
        ];
        if *self == Self::Docker {
            noops.extend_from_slice(&[
                OsStr::new("BUILDKIT_COLORS"),
                OsStr::new(BUILDKIT_HOST!()),
                OsStr::new("BUILDKIT_PROGRESS"),
                OsStr::new("BUILDKIT_TTY_LOG_LINES"),
                OsStr::new(BUILDX_BUILDER!()),
                OsStr::new("BUILDX_CPU_PROFILE"),
                OsStr::new("BUILDX_MEM_PROFILE"),
                OsStr::new(DOCKER_CONTEXT!()),
                OsStr::new("DOCKER_DEFAULT_PLATFORM"),
                OsStr::new("DOCKER_HIDE_LEGACY_COMMANDS"),
                OsStr::new(DOCKER_HOST!()),
            ]);
        }
        noops
    }

    /// Strip out flags that don't affect a build's outputs:
    pub(crate) fn buildnoop_flags(&self) -> impl Iterator<Item = &str> {
        ["--cache-from=", "--cache-to=", "--no-cache"].into_iter()
    }
}

#[test]
fn runner_is_what() {
    assert!(!Runner::Docker.is_none());
    assert!(!Runner::Podman.is_none());
    assert!(Runner::None.is_none());

    assert!(Runner::Docker.is_buildkit());
    assert!(Runner::Podman.is_buildkit());
    assert!(!Runner::None.is_buildkit());
}
