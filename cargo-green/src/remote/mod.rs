//! The remote daemon the runner sends builds to.
//!
//! * [`RemoteDaemon`]: where `$BUILDKIT_HOST` or `$DOCKER_HOST` point
//! * [`tcp`]: the TLS files of `tcp://` daemons
//!
//! TODO: podman through `$CONTAINER_HOST` or `$CONTAINER_CONNECTION`

use anyhow::{Result, anyhow, bail};
use camino::Utf8PathBuf;
use log::{info, warn};
use url::Url;

use crate::{
    all_our_envs::{
        BUILDKIT_HOST, BUILDX_BUILDER, CARGOGREEN_BUILDKIT_TLSDIR, CONTAINER_CONNECTION,
        CONTAINER_HOST, DOCKER_HOST,
    },
    redact::Vars,
    remote::tcp::{buildkit_tls, docker_tls},
    runner::Runner,
};

mod tcp;

pub(crate) use tcp::TlsFiles;

/// A daemon builds go to, from `$DOCKER_HOST` or `$BUILDKIT_HOST`
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteDaemon {
    /// A docker daemon, from `$DOCKER_HOST`, maybe on `tcp://`, maybe behind TLS
    Docker { url: Url, tls: Option<TlsFiles> },
    /// A BuildKit daemon, from `$BUILDKIT_HOST`, maybe on `tcp://`, maybe behind TLS
    BuildKit { url: Url, tls: Option<TlsFiles> },
}

impl Vars {
    /// Picks (maybe) a [RemoteDaemon] from env and the given [Runner]
    ///
    /// Without a runner, nothing gets built in a container: no daemon.
    pub(crate) fn remote_daemon(&self, runner: Runner) -> Result<Option<RemoteDaemon>> {
        let is_set = |var| self.get_nonempty(var).is_some();
        match runner {
            Runner::None => return Ok(None),
            Runner::Podman => {
                if is_set(CONTAINER_HOST!()) || is_set(CONTAINER_CONNECTION!()) {
                    let msg = format!(
                        "Neither {CONTAINER_HOST} nor {CONTAINER_CONNECTION} are supported (yet): builds run on the local podman"
                    );
                    warn!("{msg}");
                    eprintln!("{msg}");
                }
                return Ok(None);
            }
            Runner::Docker => { /* see below */ }
        }

        for var in [DOCKER_HOST!(), DOCKER_CONTEXT!(), BUILDKIT_HOST!(), BUILDX_BUILDER!()] {
            if let Some(val) = self.get(var) {
                info!("${var} is set to {val:?}");
                eprintln!("${var} is set to {val:?}");
            }
        }

        // Cf. https://docs.docker.com/build/building/variables/#buildkit_host
        if let Some(name) = self.get_nonempty(BUILDX_BUILDER!())
            && let Some(val) = self.get(BUILDKIT_HOST!())
        {
            bail!(
                "Overriding {BUILDKIT_HOST}={val:?} while setting {BUILDX_BUILDER}={name:?} is unsupported"
            )
        }

        let remote = match runner {
            Runner::None | Runner::Podman => unreachable!(),
            Runner::Docker => RemoteDaemon::from_env(self)?,
        };

        if let Some(dir) = self.get_nonempty(CARGOGREEN_BUILDKIT_TLSDIR!())
            && !remote
                .as_ref()
                .filter(|r| matches!(r, RemoteDaemon::BuildKit { .. }))
                .filter(|r| r.url().scheme() == "tcp")
                .is_some()
        {
            bail!("{CARGOGREEN_BUILDKIT_TLSDIR}={dir:?} only applies to {BUILDKIT_HOST}=tcp://…")
        }
        Ok(remote)
    }
}

/// Docker's configuration directory: `$DOCKER_CONFIG`, defaulting to `~/.docker`.
fn docker_config_dir(env: &Vars) -> Option<Utf8PathBuf> {
    if let Some(dir) = env.get_nonempty(DOCKER_CONFIG!()) {
        return Some(dir.into());
    }
    let home = Utf8PathBuf::try_from(home::home_dir()?).ok()?;
    Some(home.join(".docker"))
}

/// Rejects addresses we cannot reach builds through
fn checked(url: Url, origin: impl FnOnce() -> String) -> Result<Url> {
    if url.scheme() == "ssh" && url.host().is_none() {
        bail!("Missing host in {}", origin())
    }
    Ok(url)
}

impl RemoteDaemon {
    /// Parses `$BUILDKIT_HOST` and `$DOCKER_HOST`, along with their TLS settings
    pub(crate) fn from_env(env: &Vars) -> Result<Option<Self>> {
        let (var, addr) =
            match (env.get_nonempty(BUILDKIT_HOST!()), env.get_nonempty(DOCKER_HOST!())) {
                (Some(addr), None) => (BUILDKIT_HOST!(), addr),
                (None, Some(addr)) => (DOCKER_HOST!(), addr),
                (None, None) => return Ok(None),
                (Some(_), Some(_)) => {
                    bail!("Setting both {BUILDKIT_HOST} and {DOCKER_HOST} is unsupported")
                }
            };
        let url = Url::parse(addr).map_err(|e| anyhow!("Bad ${var}={addr:?}: {e}"))?;
        let url = checked(url, || format!("${var}={addr:?}"))?;
        Ok(Some(if var == BUILDKIT_HOST!() {
            let tls = if url.scheme() == "tcp" { buildkit_tls(env)? } else { None };
            Self::BuildKit { url, tls }
        } else {
            let tls = if url.scheme() == "tcp" { docker_tls(env, &url)? } else { None };
            Self::Docker { url, tls }
        }))
    }

    /// Its address (buildx records it verbatim as a builder's endpoint)
    pub(crate) fn url(&self) -> &Url {
        match self {
            Self::Docker { url, .. } | Self::BuildKit { url, .. } => url,
        }
    }

    /// Its TLS settings, only ever for `tcp://` addresses
    pub(crate) fn tls(&self) -> Option<&TlsFiles> {
        match self {
            Self::Docker { tls, .. } | Self::BuildKit { tls, .. } => tls.as_ref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use anyhow::Result;
    use url::Url;

    use super::{RemoteDaemon, TlsFiles};
    use crate::{
        redact::Vars,
        runner::Runner,
        sys::{Sys, fake::FakeFs},
    };

    /// Runs `f` against a fake filesystem, set up by `files`.
    fn with_fs<T>(files: impl FnOnce(&FakeFs), f: impl FnOnce() -> T) -> T {
        let fs = FakeFs::new();
        files(&fs);
        let _guard = Sys::install(Sys { fs: fs.clone() as Arc<_>, ..Sys::fake() });
        f()
    }

    fn no_files(_: &FakeFs) {}

    /// Resolves the daemon with docker
    fn resolve<const N: usize>(env: [(&str, &str); N]) -> Result<Option<RemoteDaemon>> {
        Vars::from(env).remote_daemon(Runner::Docker)
    }

    fn dckr(addr: &str) -> RemoteDaemon {
        RemoteDaemon::Docker { url: Url::parse(addr).unwrap(), tls: None }
    }

    fn bldkt(addr: &str) -> RemoteDaemon {
        RemoteDaemon::BuildKit { url: Url::parse(addr).unwrap(), tls: None }
    }

    fn daemon<const N: usize>(env: [(&str, &str); N]) -> Result<Option<RemoteDaemon>> {
        RemoteDaemon::from_env(&Vars::from(env))
    }

    #[test]
    fn podman_hosts_are_left_alone() {
        let env = Vars::from([(CONTAINER_HOST!(), "ssh://far")]);
        assert_eq!(env.remote_daemon(Runner::Podman).unwrap(), None);
    }

    #[test]
    fn builder_with_buildkit_host() {
        let env = Vars::from([(BUILDX_BUILDER!(), "mine"), (BUILDKIT_HOST!(), "tcp://h:1234")]);
        let err = env.remote_daemon(Runner::Docker).unwrap_err().to_string();
        assert!(err.contains("is unsupported"), "{err}");
    }

    #[test]
    fn builder_with_buildkit_host_without_runner() {
        let env = Vars::from([(BUILDX_BUILDER!(), "mine"), (BUILDKIT_HOST!(), "tcp://h:1234")]);
        assert_eq!(env.remote_daemon(Runner::None).unwrap(), None);
    }

    #[test]
    fn no_daemon_without_runner() {
        // Not even read: even settings that would be errors with a runner
        for env in [
            Vars::from([(DOCKER_HOST!(), "ssh://h")]),
            Vars::from([(DOCKER_HOST!(), "not a URL")]),
            Vars::from([(DOCKER_HOST!(), "ssh://a"), (BUILDKIT_HOST!(), "ssh://b")]),
            Vars::from([(DOCKER_HOST!(), "tcp://h:2376"), (DOCKER_TLS!(), "1")]),
            Vars::from([(CARGOGREEN_BUILDKIT_TLSDIR!(), "/certs")]),
        ] {
            assert_eq!(env.remote_daemon(Runner::None).unwrap(), None);
        }
    }

    #[test]
    fn no_builder_with_buildkit_host() {
        let env = Vars::from([(BUILDX_BUILDER!(), ""), (BUILDKIT_HOST!(), "tcp://h:1234")]);
        assert!(env.remote_daemon(Runner::Docker).is_ok());
    }

    #[test]
    fn builder_with_docker_host() {
        let env = Vars::from([(BUILDX_BUILDER!(), "mine"), (DOCKER_HOST!(), "ssh://h")]);
        assert!(env.remote_daemon(Runner::Docker).is_ok());
    }

    #[test]
    fn buildkit_tlsdir_without_host() {
        let env = [(CARGOGREEN_BUILDKIT_TLSDIR!(), "/certs")];
        assert!(with_fs(no_files, || resolve(env)).is_err());
    }

    #[test]
    fn buildkit_tlsdir_with_ssh_buildkit_host() {
        let env = [(CARGOGREEN_BUILDKIT_TLSDIR!(), "/certs"), (BUILDKIT_HOST!(), "ssh://h")];
        assert!(with_fs(no_files, || resolve(env)).is_err());
    }

    #[test]
    fn buildkit_tlsdir_with_tcp_docker_host() {
        let env = [(CARGOGREEN_BUILDKIT_TLSDIR!(), "/certs"), (DOCKER_HOST!(), "tcp://h:1234")];
        assert!(with_fs(no_files, || resolve(env)).is_err());
    }

    #[test]
    fn buildkit_tlsdir_with_tcp_buildkit_host() {
        let env = [(CARGOGREEN_BUILDKIT_TLSDIR!(), "/certs"), (BUILDKIT_HOST!(), "tcp://h:1234")];
        let remote = with_fs(|fs| fs.file("/certs/ca.pem", ""), || resolve(env)).unwrap();
        let tls = TlsFiles { ca: "/certs/ca.pem".into(), cert_key: None };
        let url = Url::parse("tcp://h:1234").unwrap();
        assert_eq!(remote, Some(RemoteDaemon::BuildKit { url, tls: Some(tls) }));
    }

    #[test]
    fn buildkit_tlsdir_empty_is_unset() {
        assert!(with_fs(no_files, || resolve([(CARGOGREEN_BUILDKIT_TLSDIR!(), "")])).is_ok());
    }

    #[test]
    fn no_daemon() {
        assert_eq!(daemon([]).unwrap(), None);
        assert_eq!(daemon([(DOCKER_HOST!(), ""), (BUILDKIT_HOST!(), "")]).unwrap(), None);
    }

    #[test]
    fn docker_daemon() {
        let remote = daemon([(DOCKER_HOST!(), "ssh://h")]).unwrap();
        assert_eq!(remote, Some(dckr("ssh://h")));
    }

    #[test]
    fn buildkit_daemon() {
        let remote = daemon([(BUILDKIT_HOST!(), "tcp://h:1234")]).unwrap();
        assert_eq!(remote, Some(bldkt("tcp://h:1234")));
    }

    #[test]
    fn empty_host_is_unset() {
        let remote = daemon([(DOCKER_HOST!(), ""), (BUILDKIT_HOST!(), "ssh://h")]).unwrap();
        assert_eq!(remote, Some(bldkt("ssh://h")));
    }

    #[test]
    fn both_hosts() {
        assert!(daemon([(DOCKER_HOST!(), "ssh://a"), (BUILDKIT_HOST!(), "ssh://b")]).is_err());
    }

    #[test]
    fn bad_host() {
        assert!(daemon([(DOCKER_HOST!(), "not a URL")]).is_err());
        assert!(daemon([(BUILDKIT_HOST!(), "ssh://")]).is_err());
    }
}
