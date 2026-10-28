//! The remote daemon the runner sends builds to.
//!
//! * [`RemoteDaemon`]: where `$BUILDKIT_HOST`, `$DOCKER_HOST` (or the active docker context) point
//! * [`pool`]: warm connections to it, when it is reached through `ssh://`
//! * [`ssh`]: `ssh` invocations reaching it
//! * [`tcp`]: the TLS files of `tcp://` daemons
//!
//! TODO: podman through `$CONTAINER_HOST` or `$CONTAINER_CONNECTION`

use anyhow::{Result, anyhow, bail};
use camino::Utf8PathBuf;
use log::{info, warn};
use serde::Deserialize;
use url::Url;

use crate::{
    all_our_envs::{
        BUILDKIT_HOST, BUILDX_BUILDER, CARGOGREEN_BUILDKIT_TLSDIR, CONTAINER_CONNECTION,
        CONTAINER_HOST, DOCKER_HOST,
    },
    redact::Vars,
    remote::tcp::{buildkit_tls, docker_tls},
    runner::Runner,
    sys::fs,
};

mod pool;
mod ssh;
mod tcp;

pub(crate) use pool::{Pooled, RemotePool};
pub(crate) use tcp::TlsFiles;

/// A daemon builds go to, from `$DOCKER_HOST` (or the active docker context) or `$BUILDKIT_HOST`
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteDaemon {
    /// A docker daemon, from `$DOCKER_HOST` or the active docker context, maybe on `tcp://`, maybe behind TLS
    Docker { url: Url, tls: Option<TlsFiles> },
    /// A BuildKit daemon, from `$BUILDKIT_HOST`, maybe on `tcp://`, maybe behind TLS
    BuildKit { url: Url, tls: Option<TlsFiles> },
}

impl Vars {
    /// Picks (maybe) a [RemoteDaemon] from env and the given [Runner]
    ///
    /// Without a runner, nothing gets built in a container: no daemon.
    ///
    /// With docker and neither `$DOCKER_HOST` nor `$BUILDKIT_HOST` set, docker uses the context
    /// from `$DOCKER_CONTEXT` or from its (filesystem) config's `currentContext`.
    /// When that context reaches its daemon through `ssh://` or `tcp://`, that daemon is returned
    /// (with the context's TLS settings) so builds get a builder of their own and pooled connections.
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
            Runner::Docker if !is_set(DOCKER_HOST!()) && !is_set(BUILDKIT_HOST!()) => {
                docker_context(self)?.map(DockerContext::daemon).transpose()?.flatten()
            }
            Runner::Docker => RemoteDaemon::from_env(self)?,
        };

        if let Some(dir) = self.get_nonempty(CARGOGREEN_BUILDKIT_TLSDIR!())
            && remote
                .as_ref()
                .filter(|r| matches!(r, RemoteDaemon::BuildKit { .. }))
                .filter(|r| r.url().scheme() == "tcp")
                .is_none()
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

struct DockerContext {
    name: String,
    host: String,
    skip_tls_verify: bool,
    /// Holds `ca.pem`, `cert.pem` and `key.pem`, when the context has TLS data
    tls_dir: Utf8PathBuf,
}

impl DockerContext {
    /// This context's daemon, when it is reached through `ssh://` or `tcp://`.
    fn daemon(self) -> Result<Option<RemoteDaemon>> {
        let Self { name, host, skip_tls_verify, tls_dir } = self;
        let Some(url) = Url::parse(&host).ok() else { return Ok(None) };
        let true = ["ssh", "tcp"].contains(&url.scheme()) else { return Ok(None) };
        let url = checked(url, || format!("docker context {name:?}'s {host:?}"))?;
        info!("using {DOCKER_HOST}={host:?} from docker context {name:?}");
        eprintln!("docker context {name:?} is on {host:?}");

        let mut tls = None;
        // Docker uses TLS when the context has TLS data
        if url.scheme() == "tcp" && fs().exists(&tls_dir.join("ca.pem")) {
            if skip_tls_verify {
                bail!("Unsupported docker context {name:?}: it skips TLS verification of {host:?}")
            }
            tls = Some(TlsFiles::from_docker_config_dir(&tls_dir));
        }
        Ok(Some(RemoteDaemon::Docker { url, tls }))
    }
}

/// Rejects addresses we cannot reach builds through
fn checked(url: Url, origin: impl FnOnce() -> String) -> Result<Url> {
    if url.scheme() == "ssh" && url.host().is_none() {
        bail!("Missing host in {}", origin())
    }
    Ok(url)
}

/// Reads the active docker context, the way the docker CLI resolves it.
fn docker_context(env: &Vars) -> Result<Option<DockerContext>> {
    let Some(config) = docker_config_dir(env) else { return Ok(None) };

    let name = match env.get_nonempty(DOCKER_CONTEXT!()) {
        Some(name) => name.to_owned(),
        None => {
            #[derive(Deserialize)]
            struct Config {
                #[serde(rename = "currentContext")]
                current_context: Option<String>,
            }
            let path = config.join("config.json");
            let Ok(json) = fs().read_to_string(&path) else { return Ok(None) };
            let Config { current_context } =
                serde_json::from_str(&json).map_err(|e| anyhow!("Failed reading {path}: {e}"))?;
            let Some(name) = current_context else { return Ok(None) };
            name
        }
    };
    if name.is_empty() || name == "default" {
        return Ok(None);
    }

    #[derive(Deserialize)]
    struct Meta {
        #[serde(rename = "Endpoints")]
        endpoints: Endpoints,
    }
    #[derive(Deserialize)]
    struct Endpoints {
        docker: Option<Endpoint>,
    }
    #[derive(Deserialize)]
    struct Endpoint {
        #[serde(rename = "Host")]
        host: Option<String>,
        #[serde(rename = "SkipTLSVerify", default)]
        skip_tls_verify: bool,
    }

    let id = sha256::digest(&name);
    let meta = config.join("contexts/meta").join(&id).join("meta.json");
    let json = fs()
        .read_to_string(&meta)
        .map_err(|e| anyhow!("Failed reading docker context {name:?} ({meta}): {e}"))?;
    let Meta { endpoints: Endpoints { docker } } = serde_json::from_str(&json)
        .map_err(|e| anyhow!("Failed decoding docker context {name:?} ({meta}): {e}"))?;
    let Some(Endpoint { host: Some(host), skip_tls_verify }) = docker else { return Ok(None) };
    let tls_dir = config.join("contexts/tls").join(&id).join("docker");
    Ok(Some(DockerContext { name, host, skip_tls_verify, tls_dir }))
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

    /// Which variable holds its address
    fn var(&self) -> &'static str {
        match self {
            Self::Docker { .. } => DOCKER_HOST!(),
            Self::BuildKit { .. } => BUILDKIT_HOST!(),
        }
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

    /// Whether we can pool connections to it
    fn is_poolable(&self) -> bool {
        self.url().scheme() == "ssh"
    }

    /// The socket path the remote daemon is reached through, from the remote's identity.
    /// `ssh://host/` and `ssh://host` are the same daemon, as with docker and BuildKit.
    fn path(&self) -> &str {
        match self.url().path() {
            "/" => "",
            path => path,
        }
    }

    /// Identifies this daemon: same daemon, same key.
    fn key(&self) -> String {
        let mut url = self.url().clone();
        url.set_path(self.path());
        format!("{}={url}", self.var())
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use camino::Utf8PathBuf;
    use url::Url;

    use super::{RemoteDaemon, TlsFiles};
    use crate::{
        redact::Vars,
        runner::Runner,
        sys::{Sys, fake::FakeFs},
    };

    const CONFIG: &str = "/home/me/.docker";

    /// Runs `f` against a fake filesystem, set up by `files`.
    fn with_fs<T>(files: impl FnOnce(&FakeFs), f: impl FnOnce() -> T) -> T {
        let fs = FakeFs::new();
        files(&fs);
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        f()
    }

    /// Writes a docker context, giving its TLS data directory
    fn context(fs: &FakeFs, name: &str, endpoint: &str) -> Utf8PathBuf {
        let id = sha256::digest(name);
        let json =
            format!(r#"{{"Name":"{name}","Metadata":{{}},"Endpoints":{{"docker":{endpoint}}}}}"#);
        fs.file(format!("{CONFIG}/contexts/meta/{id}/meta.json"), json);
        Utf8PathBuf::from(format!("{CONFIG}/contexts/tls/{id}/docker"))
    }

    fn far(fs: &FakeFs) {
        let _ = context(fs, "far", r#"{"Host":"ssh://me@far","SkipTLSVerify":false}"#);
    }

    /// Resolves the daemon with docker, reading docker's config from [`CONFIG`]
    fn resolve<const N: usize>(env: [(&str, &str); N]) -> Result<Option<RemoteDaemon>> {
        let env = env.into_iter().chain([(DOCKER_CONFIG!(), CONFIG)]);
        let env: Vars = env.map(|(var, val)| (var.to_owned(), val.to_owned())).collect();
        env.remote_daemon(Runner::Docker)
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
    fn no_context_no_host() {
        assert_eq!(with_fs(far, || resolve([])).unwrap(), None);
    }

    #[test]
    fn context_from_env() {
        let remote = with_fs(far, || resolve([(DOCKER_CONTEXT!(), "far")]));
        assert_eq!(remote.unwrap(), Some(dckr("ssh://me@far")));
    }

    #[test]
    fn context_default_is_local() {
        assert_eq!(with_fs(far, || resolve([(DOCKER_CONTEXT!(), "default")])).unwrap(), None);
    }

    #[test]
    fn context_overridden_by_docker_host() {
        // An explicit address wins, as with docker
        let env = [(DOCKER_CONTEXT!(), "far"), (DOCKER_HOST!(), "tcp://near:2375")];
        assert_eq!(with_fs(far, || resolve(env)).unwrap(), Some(dckr("tcp://near:2375")));
    }

    #[test]
    fn context_overridden_by_buildkit_host() {
        let env = [(DOCKER_CONTEXT!(), "far"), (BUILDKIT_HOST!(), "tcp://near:1234")];
        assert_eq!(with_fs(far, || resolve(env)).unwrap(), Some(bldkt("tcp://near:1234")));
    }

    #[test]
    fn context_from_config() {
        let files = |fs: &FakeFs| {
            far(fs);
            fs.file(format!("{CONFIG}/config.json"), r#"{"auths":{},"currentContext":"far"}"#);
        };
        assert_eq!(with_fs(files, || resolve([])).unwrap(), Some(dckr("ssh://me@far")));
    }

    #[test]
    fn context_from_config_without_current() {
        let files = |fs: &FakeFs| fs.file(format!("{CONFIG}/config.json"), r#"{"auths":{}}"#);
        assert_eq!(with_fs(files, || resolve([])).unwrap(), None);
    }

    #[test]
    fn context_from_bad_config() {
        let files = |fs: &FakeFs| fs.file(format!("{CONFIG}/config.json"), "{");
        assert!(with_fs(files, || resolve([])).is_err());
    }

    #[test]
    fn context_missing() {
        assert!(with_fs(far, || resolve([(DOCKER_CONTEXT!(), "nope")])).is_err());
    }

    #[test]
    fn context_bad_meta() {
        let files = |fs: &FakeFs| {
            let id = sha256::digest("bad");
            fs.file(format!("{CONFIG}/contexts/meta/{id}/meta.json"), "{}");
        };
        assert!(with_fs(files, || resolve([(DOCKER_CONTEXT!(), "bad")])).is_err());
    }

    #[test]
    fn context_without_docker_endpoint() {
        let files = |fs: &FakeFs| {
            let id = sha256::digest("k8s");
            let json = r#"{"Name":"k8s","Metadata":{},"Endpoints":{}}"#;
            fs.file(format!("{CONFIG}/contexts/meta/{id}/meta.json"), json);
        };
        assert_eq!(with_fs(files, || resolve([(DOCKER_CONTEXT!(), "k8s")])).unwrap(), None);
    }

    #[test]
    fn context_without_ssh_host() {
        let files = |fs: &FakeFs| {
            let _ = context(fs, "nohost", r#"{"Host":"ssh://","SkipTLSVerify":false}"#);
        };
        assert!(with_fs(files, || resolve([(DOCKER_CONTEXT!(), "nohost")])).is_err());
    }

    #[test]
    fn context_on_tcp() {
        let files = |fs: &FakeFs| {
            let _ = context(fs, "plain", r#"{"Host":"tcp://near:2375","SkipTLSVerify":false}"#);
        };
        let remote = with_fs(files, || resolve([(DOCKER_CONTEXT!(), "plain")]));
        assert_eq!(remote.unwrap(), Some(dckr("tcp://near:2375")));
    }

    #[test]
    fn context_on_tcp_with_tls() {
        let mut dir = Utf8PathBuf::new();
        let files = |fs: &FakeFs| {
            dir = context(fs, "tls", r#"{"Host":"tcp://near:2376","SkipTLSVerify":false}"#);
            fs.file(dir.join("ca.pem"), "");
        };
        let remote = with_fs(files, || resolve([(DOCKER_CONTEXT!(), "tls")])).unwrap();
        let tls = TlsFiles { ca: dir.join("ca.pem"), cert_key: None };
        let url = Url::parse("tcp://near:2376").unwrap();
        assert_eq!(remote, Some(RemoteDaemon::Docker { url, tls: Some(tls) }));
    }

    #[test]
    fn context_on_tcp_with_client_cert() {
        let mut dir = Utf8PathBuf::new();
        let files = |fs: &FakeFs| {
            dir = context(fs, "mtls", r#"{"Host":"tcp://near:2376","SkipTLSVerify":false}"#);
            fs.file(dir.join("ca.pem"), "");
            fs.file(dir.join("key.pem"), "");
        };
        let remote = with_fs(files, || resolve([(DOCKER_CONTEXT!(), "mtls")])).unwrap();
        let cert_key = Some((dir.join("cert.pem"), dir.join("key.pem")));
        assert_eq!(remote.unwrap().tls(), Some(&TlsFiles { ca: dir.join("ca.pem"), cert_key }));
    }

    #[test]
    fn context_skipping_tls_verification() {
        let files = |fs: &FakeFs| {
            let tls = context(fs, "skip", r#"{"Host":"tcp://near:2376","SkipTLSVerify":true}"#);
            fs.file(tls.join("ca.pem"), "");
        };
        assert!(with_fs(files, || resolve([(DOCKER_CONTEXT!(), "skip")])).is_err());
    }

    #[test]
    fn context_on_local_socket() {
        let files = |fs: &FakeFs| {
            let endpoint = r#"{"Host":"unix:///run/user/1000/docker.sock","SkipTLSVerify":false}"#;
            let _ = context(fs, "local", endpoint);
        };
        assert_eq!(with_fs(files, || resolve([(DOCKER_CONTEXT!(), "local")])).unwrap(), None);
    }

    #[test]
    fn context_ignored_by_other_runners() {
        for runner in [Runner::Podman, Runner::None] {
            let env = Vars::from([(DOCKER_CONTEXT!(), "far"), (DOCKER_CONFIG!(), CONFIG)]);
            assert_eq!(with_fs(far, || env.remote_daemon(runner)).unwrap(), None);
        }
    }

    #[test]
    fn podman_hosts_are_left_alone() {
        let env = Vars::from([(CONTAINER_HOST!(), "ssh://far")]);
        assert_eq!(with_fs(far, || env.remote_daemon(Runner::Podman)).unwrap(), None);
    }

    #[test]
    fn builder_with_buildkit_host() {
        let env = Vars::from([(BUILDX_BUILDER!(), "mine"), (BUILDKIT_HOST!(), "tcp://h:1234")]);
        let err = with_fs(far, || env.remote_daemon(Runner::Docker)).unwrap_err().to_string();
        assert!(err.contains("is unsupported"), "{err}");
    }

    #[test]
    fn builder_with_buildkit_host_without_runner() {
        let env = Vars::from([(BUILDX_BUILDER!(), "mine"), (BUILDKIT_HOST!(), "tcp://h:1234")]);
        assert_eq!(with_fs(far, || env.remote_daemon(Runner::None)).unwrap(), None);
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
            assert_eq!(with_fs(far, || env.remote_daemon(Runner::None)).unwrap(), None);
        }
    }

    #[test]
    fn no_builder_with_buildkit_host() {
        let env = Vars::from([(BUILDX_BUILDER!(), ""), (BUILDKIT_HOST!(), "tcp://h:1234")]);
        assert!(with_fs(far, || env.remote_daemon(Runner::Docker)).is_ok());
    }

    #[test]
    fn builder_with_docker_host() {
        let env = Vars::from([(BUILDX_BUILDER!(), "mine"), (DOCKER_HOST!(), "ssh://h")]);
        assert!(with_fs(far, || env.remote_daemon(Runner::Docker)).is_ok());
    }

    #[test]
    fn buildkit_tlsdir_without_host() {
        let env = [(CARGOGREEN_BUILDKIT_TLSDIR!(), "/certs")];
        assert!(with_fs(far, || resolve(env)).is_err());
    }

    #[test]
    fn buildkit_tlsdir_with_ssh_buildkit_host() {
        let env = [(CARGOGREEN_BUILDKIT_TLSDIR!(), "/certs"), (BUILDKIT_HOST!(), "ssh://h")];
        assert!(with_fs(far, || resolve(env)).is_err());
    }

    #[test]
    fn buildkit_tlsdir_with_tcp_docker_host() {
        let env = [(CARGOGREEN_BUILDKIT_TLSDIR!(), "/certs"), (DOCKER_HOST!(), "tcp://h:1234")];
        assert!(with_fs(far, || resolve(env)).is_err());
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
        assert!(with_fs(far, || resolve([(CARGOGREEN_BUILDKIT_TLSDIR!(), "")])).is_ok());
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
        assert_eq!(remote.unwrap().var(), DOCKER_HOST!());
    }

    #[test]
    fn buildkit_daemon() {
        let remote = daemon([(BUILDKIT_HOST!(), "tcp://h:1234")]).unwrap();
        assert_eq!(remote, Some(bldkt("tcp://h:1234")));
        assert_eq!(remote.unwrap().var(), BUILDKIT_HOST!());
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

    #[test]
    fn poolable_daemons() {
        let poolable = |addr: &str| dckr(addr).is_poolable();
        assert!(poolable("ssh://h"));
        assert!(!poolable("tcp://h:2375"));
        assert!(!poolable("unix:///var/run/docker.sock"));
    }

    #[test]
    fn daemon_keys() {
        let key = |addr: &str| dckr(addr).key();
        assert_eq!(key("ssh://h"), key("ssh://h/"));
        assert_ne!(key("ssh://h"), key("ssh://h/var/run/docker.sock"));
        assert_ne!(key("ssh://h"), key("ssh://me@h"));
        assert_ne!(key("ssh://h"), bldkt("ssh://h").key());
    }
}
