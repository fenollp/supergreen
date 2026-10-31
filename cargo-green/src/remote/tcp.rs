//! Connections to a [`RemoteDaemon`] listening on TCP, maybe behind TLS.
//!
//! Pooling these saves each connection its TCP (and TLS) handshake.
//! The pool terminates TLS: it holds the client certificates and serves plain connections
//! on its owner-only socket. That also makes TLS daemons usable by buildx builders,
//! whose `docker-container` driver ignores `$DOCKER_TLS_VERIFY` for a `tcp://` endpoint.
//!
//! ```text
//!   runner (docker, buildx, buildctl)       `cargo green`'s pool             ┊  remote host
//!                                                                            ┊
//!   client ─── unix://…/pool-<hash>.sock ──▶ accept ──▶ link ═══ TCP+TLS ════╪═══▶ daemon
//!   client ─── plain bytes ────────────────▶ accept ──▶ link ═══ TCP+TLS ════╪═══▶ daemon
//!                                                       link ═══ TCP+TLS ════╪═══▶ (spare)
//!                                                                            ┊
//!   ─── owner-only UNIX socket: no TLS
//!   ═══ handshaken ahead of time, with ca/cert/key held by the pool;
//!       spares idle over 30s are dropped, not handed out
//! ```
//!
//! Docker defaults to port 2376 with TLS, 2375 without. BuildKit has no default port.
//!
//! `$DOCKER_HOST=tcp://…` TLS settings are the docker CLI's
//! (see <https://docs.docker.com/engine/security/protect-access/>):
//! * `$DOCKER_TLS_VERIFY` (any non-empty value): verify the daemon, with `ca.pem`
//!   and present `cert.pem` & `key.pem` (when found)
//! * from `$DOCKER_CERT_PATH`, defaulting to `$DOCKER_CONFIG` then `~/.docker`
//! * `$DOCKER_TLS` alone (no verification) is not supported
//!
//! `$BUILDKIT_HOST=tcp://…` TLS settings come from `$CARGOGREEN_BUILDKIT_TLSDIR`, read like
//! `buildctl --tlsdir`: BuildKit takes these as flags only (`buildctl`, the `remote` driver's options).
use std::{sync::Arc, time::Duration};

use anyhow::{Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use rustls::{
    ClientConfig, RootCertStore,
    pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject},
};
use tokio::{io::split, net::TcpStream};
use tokio_rustls::TlsConnector;
use url::{Host, Url};

use crate::{
    all_our_envs::{
        CARGOGREEN_BUILDKIT_TLSDIR, DOCKER_CERT_PATH, DOCKER_HOST, DOCKER_TLS, DOCKER_TLS_VERIFY,
    },
    redact::Vars,
    remote::{RemoteDaemon, docker_config_dir, pool::Link},
    sys::fs,
};

/// Daemons may drop connections that stay idle: never hand out spares older than this.
/// (e.g. BuildKit's gRPC server waits 120s at most for a client's first bytes)
const MAX_IDLE: Duration = Duration::from_secs(30);

pub(super) struct Tcp {
    host: String,
    port: u16,
    tls: Option<(TlsConnector, ServerName<'static>)>,
}

impl Tcp {
    pub(super) fn new(remote: &RemoteDaemon) -> Result<Self> {
        let (url, var) = (remote.url(), remote.var());
        let Some(host) = url.host() else { bail!("Missing host in ${var}={url}") };
        let port = match (url.port(), remote) {
            (Some(port), _) => port,
            (None, RemoteDaemon::Docker { tls: Some(_), .. }) => 2376,
            (None, RemoteDaemon::Docker { tls: None, .. }) => 2375,
            (None, RemoteDaemon::BuildKit { .. }) => bail!("Missing port in ${var}={url}"),
        };
        let (host, name): (_, ServerName) = match host {
            Host::Domain(domain) => (
                domain.to_owned(),
                domain
                    .to_owned()
                    .try_into()
                    .map_err(|e| anyhow!("Invalid DNS name {domain}: {e}"))?,
            ),
            Host::Ipv4(ip) => (ip.to_string(), ip.into()),
            Host::Ipv6(ip) => (ip.to_string(), ip.into()), // Without brackets
        };

        let tls = remote.tls();
        let tls = tls.map(TlsFiles::config);
        let tls = tls.transpose().map_err(|e| anyhow!("Invalid TLS config for {name:?}: {e}"))?;
        let tls = tls.map(|cfg| (TlsConnector::from(cfg), name));
        Ok(Self { host, port, tls })
    }

    /// A fresh connection, through the TLS handshake if any.
    pub(super) async fn connect(&self) -> Result<Link> {
        let Self { host, port, tls } = self;
        let tcp = TcpStream::connect((host.as_str(), *port))
            .await
            .map_err(|e| anyhow!("Failed connecting to {host}:{port}: {e}"))?;
        tcp.set_nodelay(true)?;
        let link = match tls {
            None => {
                let (r, w) = tcp.into_split();
                Link::new(r, w)
            }
            Some((connector, name)) => {
                let tls = connector
                    .connect(name.clone(), tcp)
                    .await
                    .map_err(|e| anyhow!("Failed TLS handshake with {host}:{port}: {e}"))?;
                // Shutting down the write half sends close_notify then FIN
                let (r, w) = split(tls);
                Link::new(r, w)
            }
        };
        Ok(link.stale_after(MAX_IDLE))
    }
}

/// A CA to verify the daemon with, maybe with a client certificate and key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TlsFiles {
    pub(crate) ca: Utf8PathBuf,
    pub(crate) cert_key: Option<(Utf8PathBuf, Utf8PathBuf)>,
}

/// Docker's TLS settings, for `url`.
pub(super) fn docker_tls(env: &Vars, url: &Url) -> Result<Option<TlsFiles>> {
    if env.get_nonempty(DOCKER_TLS_VERIFY!()).is_none() {
        if env.get_nonempty(DOCKER_TLS!()).is_some() {
            bail!("Unsupported {DOCKER_TLS} without {DOCKER_TLS_VERIFY} for {DOCKER_HOST}={url}")
        }
        return Ok(None);
    }
    let dir = env.get_nonempty(DOCKER_CERT_PATH!()).map(Utf8PathBuf::from);
    let Some(dir) = dir.or_else(|| docker_config_dir(env)) else {
        bail!("Missing {DOCKER_CERT_PATH}")
    };
    Ok(Some(TlsFiles::from_docker_config_dir(&dir)))
}

/// `$CARGOGREEN_BUILDKIT_TLSDIR`'s files, found the way `buildctl --tlsdir` does:
/// `ca.pem`, `cert.pem` and `key.pem`, or `ca.crt`, `tls.crt` and `tls.key`.
pub(super) fn buildkit_tls(env: &Vars) -> Result<Option<TlsFiles>> {
    let Some(dir) = env.get_nonempty(CARGOGREEN_BUILDKIT_TLSDIR!()) else { return Ok(None) };
    let dir: Utf8PathBuf = dir.into();
    let find =
        |names: [_; _]| names.into_iter().map(|name| dir.join(name)).find(|p| fs().exists(p));
    let Some(ca) = find(["ca.pem", "ca.crt"]) else {
        bail!("{CARGOGREEN_BUILDKIT_TLSDIR}={dir:?} has neither ca.pem nor ca.crt")
    };
    let cert_key = match (find(["cert.pem", "tls.crt"]), find(["key.pem", "tls.key"])) {
        (Some(cert), Some(key)) => Some((cert, key)),
        (None, None) => None,
        _ => {
            bail!("{CARGOGREEN_BUILDKIT_TLSDIR}={dir:?} is missing a client certificate or its key")
        }
    };
    Ok(Some(TlsFiles { ca, cert_key }))
}

impl TlsFiles {
    /// The files docker reads in `dir`: `ca.pem`, and `cert.pem` & `key.pem` when found
    pub(super) fn from_docker_config_dir(dir: &Utf8Path) -> Self {
        let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
        // NOTE: "||" is intended instead of "&&"
        let cert_key = (fs().exists(&cert) || fs().exists(&key)).then_some((cert, key));
        Self { ca: dir.join("ca.pem"), cert_key }
    }

    fn config(&self) -> Result<Arc<ClientConfig>> {
        let read = |path: &Utf8PathBuf| {
            fs().read_to_string(path).map_err(|e| anyhow!("Failed reading (RO) {path}: {e}"))
        };
        let Self { ca, cert_key } = self;

        let mut roots = RootCertStore::empty();
        for der in CertificateDer::pem_slice_iter(read(ca)?.as_bytes()) {
            let der = der.map_err(|e| anyhow!("Bad CA {ca}: {e}"))?;
            roots.add(der).map_err(|e| anyhow!("Bad CA {ca}: {e}"))?;
        }

        let config = ClientConfig::builder().with_root_certificates(roots);
        let config = if let Some((cert, key)) = cert_key {
            let certs = CertificateDer::pem_slice_iter(read(cert)?.as_bytes())
                .collect::<Result<_, _>>()
                .map_err(|e| anyhow!("Bad cert {cert}: {e}"))?;
            let key = PrivateKeyDer::from_pem_slice(read(key)?.as_bytes())
                .map_err(|e| anyhow!("Bad key {key}: {e}"))?;
            config.with_client_auth_cert(certs, key)?
        } else {
            config.with_no_client_auth()
        };
        Ok(Arc::new(config))
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use url::Url;

    use super::{Tcp, TlsFiles, buildkit_tls, docker_tls};
    use crate::{
        redact::Vars,
        remote::RemoteDaemon,
        sys::{Sys, fake::FakeFs},
    };

    fn with_fs<T>(files: impl FnOnce(&FakeFs), f: impl FnOnce() -> T) -> T {
        let fs = FakeFs::new();
        files(&fs);
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        f()
    }

    fn no_files(_: &FakeFs) {}

    fn tcp<const N: usize>(env: [(&str, &str); N]) -> Result<(String, u16, bool)> {
        let remote = RemoteDaemon::from_env(&Vars::from(env))?.unwrap();
        Tcp::new(&remote).map(|Tcp { host, port, tls }| (host, port, tls.is_some()))
    }

    #[test]
    fn docker_default_port() {
        let host_port_tls = with_fs(no_files, || tcp([(DOCKER_HOST!(), "tcp://h")]));
        assert_eq!(host_port_tls.unwrap(), ("h".to_owned(), 2375, false));
    }

    #[test]
    fn docker_port() {
        let host_port_tls = with_fs(no_files, || tcp([(DOCKER_HOST!(), "tcp://h:1234")]));
        assert_eq!(host_port_tls.unwrap(), ("h".to_owned(), 1234, false));
    }

    #[test]
    fn ipv6_without_brackets() {
        let host_port_tls = with_fs(no_files, || tcp([(DOCKER_HOST!(), "tcp://[::1]:1234")]));
        assert_eq!(host_port_tls.unwrap(), ("::1".to_owned(), 1234, false));
    }

    #[test]
    fn buildkit_port() {
        let host_port_tls = with_fs(no_files, || tcp([(BUILDKIT_HOST!(), "tcp://h:1234")]));
        assert_eq!(host_port_tls.unwrap(), ("h".to_owned(), 1234, false));
    }

    #[test]
    fn buildkit_has_no_default_port() {
        assert!(with_fs(no_files, || tcp([(BUILDKIT_HOST!(), "tcp://h")])).is_err());
    }

    #[test]
    fn buildkit_ignores_docker_tls() {
        let env = [(BUILDKIT_HOST!(), "tcp://h:1234"), (DOCKER_TLS_VERIFY!(), "1")];
        assert_eq!(with_fs(no_files, || tcp(env)).unwrap(), ("h".to_owned(), 1234, false));
    }

    #[test]
    fn buildkit_tlsdir_without_ca() {
        let env = [(BUILDKIT_HOST!(), "tcp://h:1234"), (CARGOGREEN_BUILDKIT_TLSDIR!(), "/certs")];
        let err = with_fs(no_files, || tcp(env)).unwrap_err();
        assert!(err.to_string().contains("neither ca.pem nor ca.crt"), "{err}");
    }

    #[test]
    fn buildkit_tlsdir_with_ca() {
        let env = [(BUILDKIT_HOST!(), "tcp://h:1234"), (CARGOGREEN_BUILDKIT_TLSDIR!(), "/certs")];
        let host_port_tls = with_fs(|fs| fs.file("/certs/ca.crt", ""), || tcp(env));
        assert_eq!(host_port_tls.unwrap(), ("h".to_owned(), 1234, true));
    }

    #[test]
    fn docker_tls_alone() {
        let env = [(DOCKER_HOST!(), "tcp://h"), (DOCKER_TLS!(), "1")];
        assert!(with_fs(no_files, || tcp(env)).is_err());
    }

    #[test]
    fn docker_tls_without_ca() {
        let env = [
            (DOCKER_HOST!(), "tcp://h"),
            (DOCKER_TLS_VERIFY!(), "1"),
            (DOCKER_CERT_PATH!(), "/certs"),
        ];
        let err = with_fs(no_files, || tcp(env)).unwrap_err();
        assert!(err.to_string().contains("/certs/ca.pem"), "{err}");
    }

    #[test]
    fn docker_tls_default_port() {
        let env = [
            (DOCKER_HOST!(), "tcp://h"),
            (DOCKER_TLS_VERIFY!(), "1"),
            (DOCKER_CERT_PATH!(), "/certs"),
        ];
        let host_port_tls = with_fs(|fs| fs.file("/certs/ca.pem", ""), || tcp(env));
        assert_eq!(host_port_tls.unwrap(), ("h".to_owned(), 2376, true));
    }

    #[test]
    fn docker_tls_with_bad_client_cert() {
        let env = [
            (DOCKER_HOST!(), "tcp://h"),
            (DOCKER_TLS_VERIFY!(), "1"),
            (DOCKER_CERT_PATH!(), "/certs"),
        ];
        let files = |fs: &FakeFs| {
            fs.file("/certs/ca.pem", "");
            fs.file("/certs/cert.pem", "");
        };
        let err = with_fs(files, || tcp(env)).unwrap_err();
        assert!(err.to_string().contains("/certs/key.pem"), "{err}");
    }

    #[test]
    fn docker_tls_files_from_docker_config() {
        let env = [(DOCKER_TLS_VERIFY!(), "1"), (DOCKER_CONFIG!(), "/docker")];
        let url = Url::parse("tcp://h").unwrap();
        let got = with_fs(no_files, || docker_tls(&Vars::from(env), &url));
        assert_eq!(got.unwrap(), Some(TlsFiles { ca: "/docker/ca.pem".into(), cert_key: None }));
    }

    #[test]
    fn docker_tls_files_with_client_cert() {
        let env = [(DOCKER_TLS_VERIFY!(), "1"), (DOCKER_CERT_PATH!(), "/certs")];
        let url = Url::parse("tcp://h").unwrap();
        let got =
            with_fs(|fs| fs.file("/certs/key.pem", ""), || docker_tls(&Vars::from(env), &url));
        let cert_key = Some(("/certs/cert.pem".into(), "/certs/key.pem".into()));
        assert_eq!(got.unwrap(), Some(TlsFiles { ca: "/certs/ca.pem".into(), cert_key }));
    }

    fn tlsdir(files: impl FnOnce(&FakeFs)) -> Result<Option<TlsFiles>> {
        let env = Vars::from([(CARGOGREEN_BUILDKIT_TLSDIR!(), "/certs")]);
        with_fs(files, || buildkit_tls(&env))
    }

    fn files(names: &[&str]) -> impl FnOnce(&FakeFs) {
        move |fs| names.iter().for_each(|name| fs.file(format!("/certs/{name}"), ""))
    }

    #[test]
    fn tlsdir_unset() {
        let got = with_fs(no_files, || buildkit_tls(&Vars::default()));
        assert_eq!(got.unwrap(), None);
    }

    #[test]
    fn tlsdir_without_ca() {
        assert!(tlsdir(no_files).is_err());
    }

    #[test]
    fn tlsdir_ca_only() {
        let got = tlsdir(files(&["ca.crt"])).unwrap();
        assert_eq!(got, Some(TlsFiles { ca: "/certs/ca.crt".into(), cert_key: None }));
    }

    #[test]
    fn tlsdir_cert_without_key() {
        assert!(tlsdir(files(&["ca.crt", "tls.crt"])).is_err());
    }

    #[test]
    fn tlsdir_key_without_cert() {
        assert!(tlsdir(files(&["ca.crt", "tls.key"])).is_err());
    }

    #[test]
    fn tlsdir_crt_names() {
        let got = tlsdir(files(&["ca.crt", "tls.crt", "tls.key"])).unwrap();
        let cert_key = Some(("/certs/tls.crt".into(), "/certs/tls.key".into()));
        assert_eq!(got, Some(TlsFiles { ca: "/certs/ca.crt".into(), cert_key }));
    }

    #[test]
    fn tlsdir_prefers_pem_names() {
        // As buildctl
        let names = ["ca.crt", "tls.crt", "tls.key", "ca.pem", "cert.pem", "key.pem"];
        let got = tlsdir(files(&names)).unwrap();
        let cert_key = Some(("/certs/cert.pem".into(), "/certs/key.pem".into()));
        assert_eq!(got, Some(TlsFiles { ca: "/certs/ca.pem".into(), cert_key }));
    }

    #[test]
    fn tls_config_from_bad_ca() {
        let tls = TlsFiles { ca: "/certs/ca.pem".into(), cert_key: None };
        let pem = "-----BEGIN CERTIFICATE-----\nnope\n-----END CERTIFICATE-----\n";
        let err = with_fs(|fs| fs.file("/certs/ca.pem", pem), || tls.config()).unwrap_err();
        assert!(err.to_string().contains("/certs/ca.pem"), "{err}");
    }
}
