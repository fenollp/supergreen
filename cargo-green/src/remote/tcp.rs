//! The TLS files of a [`RemoteDaemon`] listening on TCP.
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
use anyhow::{Result, bail};
use camino::{Utf8Path, Utf8PathBuf};
use url::Url;

use crate::{
    all_our_envs::{
        CARGOGREEN_BUILDKIT_TLSDIR, DOCKER_CERT_PATH, DOCKER_HOST, DOCKER_TLS, DOCKER_TLS_VERIFY,
    },
    redact::Vars,
    remote::docker_config_dir,
    sys::fs,
};

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
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use url::Url;

    use super::{TlsFiles, buildkit_tls, docker_tls};
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

    /// Whether TLS is used
    fn tls<const N: usize>(env: [(&str, &str); N]) -> Result<bool> {
        let remote = RemoteDaemon::from_env(&Vars::from(env))?.unwrap();
        Ok(remote.tls().is_some())
    }

    #[test]
    fn buildkit_ignores_docker_tls() {
        let env = [(BUILDKIT_HOST!(), "tcp://h:1234"), (DOCKER_TLS_VERIFY!(), "1")];
        assert!(!with_fs(no_files, || tls(env)).unwrap());
    }

    #[test]
    fn buildkit_tlsdir_without_ca() {
        let env = [(BUILDKIT_HOST!(), "tcp://h:1234"), (CARGOGREEN_BUILDKIT_TLSDIR!(), "/certs")];
        let err = with_fs(no_files, || tls(env)).unwrap_err();
        assert!(err.to_string().contains("neither ca.pem nor ca.crt"), "{err}");
    }

    #[test]
    fn buildkit_tlsdir_with_ca() {
        let env = [(BUILDKIT_HOST!(), "tcp://h:1234"), (CARGOGREEN_BUILDKIT_TLSDIR!(), "/certs")];
        assert!(with_fs(|fs| fs.file("/certs/ca.crt", ""), || tls(env)).unwrap());
    }

    #[test]
    fn docker_tls_alone() {
        let env = [(DOCKER_HOST!(), "tcp://h"), (DOCKER_TLS!(), "1")];
        assert!(with_fs(no_files, || tls(env)).is_err());
    }

    #[test]
    fn docker_tls_with_ca() {
        let env = [
            (DOCKER_HOST!(), "tcp://h"),
            (DOCKER_TLS_VERIFY!(), "1"),
            (DOCKER_CERT_PATH!(), "/certs"),
        ];
        assert!(with_fs(|fs| fs.file("/certs/ca.pem", ""), || tls(env)).unwrap());
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
}
