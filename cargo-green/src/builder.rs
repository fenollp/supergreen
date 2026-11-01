use std::{convert::Infallible, ffi::OsStr, fmt, str::FromStr, sync::LazyLock, time::Duration};

use anyhow::{Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexSet;
use log::info;
use serde::{Deserialize, Serialize};
use serde_with::{DeserializeFromStr, SerializeDisplay};
use url::{Host, Url};
use version_compare::Version;

use crate::{
    all_our_envs::{BUILDKIT_HOST, BUILDX_BUILDER, BUILDX_CONFIG, CARGOGREEN_BUILDER_IMAGE},
    buildkitd,
    green::Green,
    image_uri::{ImageUri, fetch_digest},
    remote::{Pooled, RemoteDaemon, TlsFiles, shadow_buildx_config, shadow_dir},
    retrier::Retrier,
    sys::fs,
    tmp,
};

/// TODO: move to `:rootless`
static BUILDKIT_IMAGE: LazyLock<ImageUri> =
    LazyLock::new(|| ImageUri::try_new("docker-image://docker.io/moby/buildkit:latest").unwrap());

/// Not a Release Candidate
///
/// <https://github.com/moby/buildkit/tags>
static LATEST_BUILDKIT: LazyLock<Version> =
    LazyLock::new(|| Version::from(include_str!("../latest_buildkit.txt").trim()).unwrap());

#[test]
fn uses_version_newer_or_equal_to() {
    assert!(Version::from("2").is_some_and(|ref v| v >= &LATEST_BUILDKIT));
}

/// Name of the builder we manage when building with the local daemon.
const MANAGED: &str = "supergreen";

/// The builder we manage.
/// A buildx builder is bound to the endpoint it was created against.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Managed {
    name: String,
    driver: Driver,
    endpoint: Option<Url>,
    /// For a `tcp://` endpoint
    tls: Option<TlsFiles>,
}

impl Default for Managed {
    fn default() -> Self {
        Self {
            name: MANAGED.to_owned(),
            driver: Driver::DockerContainer,
            endpoint: None,
            tls: None,
        }
    }
}

impl RemoteDaemon {
    /// * `$BUILDKIT_HOST` (any scheme): a `remote` builder for that BuildKit daemon
    /// * `$DOCKER_HOST`: a `docker-container` builder on that daemon, named after its address
    /// * `$DOCKER_HOST` set to the default local socket: same as unset, see [Managed::default]
    ///
    /// Remote daemons are reached through our connection pool, when there is one.
    fn managed(&self, pooled: Option<&Url>) -> Managed {
        let url = self.url();
        let (driver, modifier) = match self {
            Self::Docker { .. }
                if (url.scheme(), url.path()) == ("unix", "/var/run/docker.sock") =>
            {
                return Managed::default();
            }
            Self::Docker { .. } => (Driver::DockerContainer, ""),
            Self::BuildKit { .. } => (Driver::Remote, "-buildkit"),
        };

        let suffix: String = url[url::Position::BeforeUsername..]
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || "_.-".contains(c) { c } else { '_' })
            .collect();
        Managed {
            name: format!("{MANAGED}{modifier}-{}", suffix.trim_matches('_')),
            driver,
            endpoint: Some(pooled.unwrap_or(url).clone()),
            // The pool handles TLS
            tls: if pooled.is_some() { None } else { self.tls().cloned() },
        }
    }
}

#[cfg(test)]
mod craft {
    use url::Url;

    use super::RemoteDaemon;
    use crate::{
        builder::{Driver, Managed},
        redact::Vars,
    };

    fn url(addr: &str) -> Url {
        Url::parse(addr).unwrap()
    }

    fn mgd(addr: &str) -> Managed {
        RemoteDaemon::from_env(&Vars::from([(DOCKER_HOST!(), addr)]))
            .unwrap()
            .unwrap()
            .managed(None)
    }

    /// buildx records endpoints verbatim and existing builders get matched against ours:
    /// typical addresses must come out as given, else builders get re-created.
    #[test_case::test_case("ssh://h"; "ssh no path")]
    #[test_case::test_case("ssh://h/"; "ssh root path")]
    #[test_case::test_case("ssh://Me@BIG:2222/run/docker.sock"; "ssh with all")]
    #[test_case::test_case("tcp://10.0.0.2:2375"; "tcp ipv4")]
    #[test_case::test_case("tcp://[::1]:2375"; "tcp ipv6")]
    #[test_case::test_case("tcp://Host.Example:2376"; "tcp mixed case")]
    #[test_case::test_case("unix:///run/user/1000/docker.sock"; "unix")]
    fn managed_endpoints_are_verbatim(addr: &str) {
        assert_eq!(mgd(addr).endpoint.as_ref().map(Url::as_str), Some(addr));
    }

    /// Where parsing normalizes an address, the builder keeps its name (derived from the
    /// normalized address too): it gets re-created once at most, never duplicated.
    #[test_case::test_case("tcp://h:02375", "tcp://h:2375"; "port leading zeros")]
    #[test_case::test_case("tcp://[0:0::1]:2375", "tcp://[::1]:2375"; "ipv6 uncompressed")]
    #[test_case::test_case(" ssh://h ", "ssh://h"; "surrounding spaces")]
    fn managed_endpoints_normalized(addr: &str, normalized: &str) {
        assert_eq!(mgd(addr).endpoint.as_ref().map(Url::as_str), Some(normalized));
        assert_eq!(mgd(addr).name, mgd(normalized).name);
    }

    #[test]
    fn managed_builders() {
        fn with_sock<const N: usize>(env: [(&str, &str); N], sock: Option<&Url>) -> Managed {
            let remote = RemoteDaemon::from_env(&Vars::from(env)).unwrap();
            remote.map(|d| d.managed(sock)).unwrap_or_default()
        }

        let sock = url("unix:///run/user/1000/cargo-green/pool-1234.sock");
        for (env, name) in [
            ([(DOCKER_HOST!(), "ssh://big")], "supergreen-big"),
            ([(BUILDKIT_HOST!(), "ssh://big")], "supergreen-buildkit-big"),
        ] {
            let ours = with_sock(env, Some(&sock));
            assert_eq!((ours.name.as_str(), ours.endpoint.as_ref()), (name, Some(&sock)));
        }

        fn with<const N: usize>(env: [(&str, &str); N]) -> Managed {
            with_sock(env, None)
        }

        assert_eq!(with([]), Managed::default());
        assert_eq!(with([(DOCKER_HOST!(), "unix:///var/run/docker.sock")]), Managed::default());
        assert_eq!(
            with([(DOCKER_HOST!(), "unix:///run/user/1000/docker.sock")]),
            Managed {
                name: "supergreen-run_user_1000_docker.sock".into(),
                driver: Driver::DockerContainer,
                endpoint: Some(url("unix:///run/user/1000/docker.sock")),
                tls: None,
            }
        );
        assert_eq!(
            with([(BUILDKIT_HOST!(), "unix:///run/buildkit/buildkitd.sock")]),
            Managed {
                name: "supergreen-buildkit-run_buildkit_buildkitd.sock".into(),
                driver: Driver::Remote,
                endpoint: Some(url("unix:///run/buildkit/buildkitd.sock")),
                tls: None,
            }
        );
        assert_eq!(
            with([(DOCKER_HOST!(), "ssh://BIG")]),
            Managed {
                name: "supergreen-BIG".into(),
                driver: Driver::DockerContainer,
                endpoint: Some(url("ssh://BIG")),
                tls: None,
            }
        );
        assert_eq!(
            with([(DOCKER_HOST!(), "ssh://BIG/var/run/docker.sock")]),
            Managed {
                name: "supergreen-BIG_var_run_docker.sock".into(),
                driver: Driver::DockerContainer,
                endpoint: Some(url("ssh://BIG/var/run/docker.sock")),
                tls: None,
            }
        );
        assert_eq!(
            with([(BUILDKIT_HOST!(), "unix:///var/run/docker.sock")]),
            Managed {
                name: "supergreen-buildkit-var_run_docker.sock".into(),
                driver: Driver::Remote,
                endpoint: Some(url("unix:///var/run/docker.sock")),
                tls: None,
            }
        );
        assert_eq!(
            with([(DOCKER_HOST!(), "tcp://10.0.0.2:2375")]),
            Managed {
                name: "supergreen-10.0.0.2_2375".into(),
                driver: Driver::DockerContainer,
                endpoint: Some(url("tcp://10.0.0.2:2375")),
                tls: None,
            }
        );
        assert_eq!(
            with([(BUILDKIT_HOST!(), "ssh://me@ter/run/bk.sock")]),
            Managed {
                name: "supergreen-buildkit-me_ter_run_bk.sock".into(),
                driver: Driver::Remote,
                endpoint: Some(url("ssh://me@ter/run/bk.sock")),
                tls: None,
            }
        );
    }
}

#[derive(Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct Builder {
    #[doc = envdocs!(BUILDX_BUILDER)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "builder-name")]
    pub(crate) name: Option<String>,

    #[doc = envdocs!(CARGOGREEN_BUILDER_IMAGE)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "builder-image")]
    pub(crate) image: Option<ImageUri>,

    /// Shows which driver the configured builder uses.
    ///
    /// Defaults to [Driver::Docker].
    ///
    /// See <https://docs.docker.com/build/drivers/>
    /// * <https://docs.docker.com/build/drivers/docker-container/>
    /// * <https://docs.docker.com/build/drivers/remote/>
    /// * <https://docs.docker.com/build/drivers/kubernetes/>
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "builder-driver")]
    pub(crate) driver: Option<Driver>,

    /// Builder container ID
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "id")]
    pub(crate) id: Option<String>,

    /// Builder container data directory
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "data")]
    pub(crate) data: Option<Utf8PathBuf>,
}

impl Builder {
    pub(crate) fn is_default(&self) -> bool {
        self.driver.as_ref().is_none_or(|d| *d == Driver::default())
    }
}

/// <https://docs.docker.com/build/builders/drivers/>
#[derive(Clone, Debug, Default, PartialEq, Eq, SerializeDisplay, DeserializeFromStr)]
pub(crate) enum Driver {
    /// <https://docs.docker.com/build/builders/drivers/docker/>
    #[default]
    Docker,
    /// <https://docs.docker.com/build/builders/drivers/docker-container/#qemu>
    DockerContainer,
    /// <https://docs.docker.com/build/builders/drivers/remote/>
    Remote,
    /// Not an exhaustive list: future-proof (e.g. `"kubernetes"`)
    Other(String),
}

/// As `docker buildx` names them
impl fmt::Display for Driver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Docker => "docker",
            Self::DockerContainer => "docker-container",
            Self::Remote => "remote",
            Self::Other(driver) => driver,
        })
    }
}

impl FromStr for Driver {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "docker" => Self::Docker,
            "docker-container" => Self::DockerContainer,
            "remote" => Self::Remote,
            _ => Self::Other(s.to_owned()),
        })
    }
}

impl Green {
    /// The builder we manage, given where builds go
    pub(crate) fn managed(&self, remote: Option<&RemoteDaemon>) -> Managed {
        let pooled = self.pool.as_ref().map(|Pooled { addr, .. }| addr);
        remote.map(|d| d.managed(pooled)).unwrap_or_default()
    }

    /// Lets builders we do not manage use our connection pool, see [`shadow_buildx_config`].
    fn maybe_shadow_buildx_config(&mut self, remote: Option<&RemoteDaemon>) -> Result<()> {
        let (Some(remote), Some(Pooled { addr, .. }), Some(dirs)) =
            (remote, &self.pool, &self.paths.dirs)
        else {
            return Ok(());
        };
        let shadow = shadow_dir(&dirs.var_run);
        if shadow_buildx_config(&self.env, remote, addr.as_str(), &shadow)? {
            info!("using {BUILDX_CONFIG}={shadow}");
            self.buildx_config = Some(shadow);
        }
        Ok(())
    }

    pub(crate) async fn maybe_inspect_builder(&mut self) -> Result<()> {
        let Some(ref name) = self.builder.name else { return Ok(()) };
        let Some(Driver::DockerContainer) = self.builder.driver else { return Ok(()) };

        let mut cmd = self.cmd()?;
        // TODO: find out how to come up with "buildx_buildkit_supergreen0"
        cmd.arg("inspect").arg(format!("buildx_buildkit_{name}0"));

        let (succeeded, stdout, stderr) = cmd.exec().await?;
        if !succeeded {
            let stderr = String::from_utf8_lossy(&stderr);
            bail!("Failed to inspect {BUILDX_BUILDER}={name:?}'s container: maybe that builder was never started?
Try `docker buildx inspect --bootstrap {name}`
{stderr}")
        }

        let stdout = String::from_utf8_lossy(&stdout);
        let inspect: Vec<DockerInspect> = serde_json::from_str(&stdout)
            .map_err(|e| anyhow!("Failed decoding docker inspect: {e}"))?;
        if let Some(DockerInspect { id, mounts }) = inspect.into_iter().next()
            && !id.is_empty()
        {
            self.builder.id = Some(id);
            if let Some(InspectMount { source }) = mounts.into_iter().next() {
                self.builder.data = Some(source);
            }
        }

        Ok(())
    }

    pub(crate) async fn maybe_setup_builder(
        &mut self,
        env: Option<&str>,
        remote: Option<&RemoteDaemon>,
    ) -> Result<()> {
        if self.runner.is_none() {
            info!("Skipping builder setup (runner:{})", self.runner);
            return Ok(());
        }

        let ours = self.managed(remote);
        let (managed, name) = match env {
            None | Some(MANAGED) => (true, ours.name.as_str()),
            Some("") => {
                if let Some(ref img) = self.builder.image {
                    bail!("Not using a builder, however {CARGOGREEN_BUILDER_IMAGE}={img:?} is set")
                }
                // buildx's current builder may be one of the user's
                return self.maybe_shadow_buildx_config(remote);
            }
            Some(name) => (false, name),
        };
        if !managed {
            self.maybe_shadow_buildx_config(remote)?; // Before listing: shows the pooled endpoints
        }

        let builders = self.list_builders().await?;
        let builder = find_builder(name, &builders);
        info!("found builder {builder:?}");
        let pooled = self.pool.as_ref().map(|Pooled { var, .. }| var.as_str());
        let wanted = Wanted { name, managed, ours: &ours, image: self.builder.image.as_ref() };
        let (chosen, advice) = wanted.setup(builder, pooled)?;
        for msg in advice {
            eprintln!("{msg}");
        }
        match chosen {
            Setup::Use => {}
            Setup::Create => self.create_builder(name, &ours).await?,
            Setup::Recreate => {
                self.remove_builder(name, true).await?;
                self.create_builder(name, &ours).await?;
            }
        }

        if self.builder.image.is_none() {
            // Only informational: only used through showing envs values
            self.builder.image = builder.and_then(BuildxBuilder::first_image);
        }
        self.builder.driver =
            if managed { Some(ours.driver) } else { builder.map(|bb| bb.driver.clone()) };
        self.builder.name = Some(name.to_owned());
        Ok(())
    }

    pub(crate) async fn remove_builder(&mut self, name: &str, keep_state: bool) -> Result<()> {
        if !keep_state {
            return self.try_removing_builder(name, false).await;
        }
        // First try keeping state...
        if self.try_removing_builder(name, true).await.is_err() {
            // ...then stop messing about.
            self.try_removing_builder(name, false).await?;
        }
        Ok(())
    }

    pub(crate) async fn create_builder(&mut self, name: &str, ours: &Managed) -> Result<()> {
        if let Some(args) = ours.remote_args(name) {
            if let Some(ref img) = self.builder.image {
                bail!("{CARGOGREEN_BUILDER_IMAGE}={img:?} cannot apply to {BUILDKIT_HOST}'s daemon")
            }
            return self.create_builder_with(args).await;
        }

        let hosts = self
            .cache
            .from_images
            .iter()
            .chain(self.cache.to_images.iter())
            .chain(self.cache.images.iter())
            .map(ImageUri::host)
            .collect::<IndexSet<_>>();

        let mut clt = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(4))
            .build()
            .map_err(|e| anyhow!("Cannot create HTTP(s) client: {e}"))?;

        async fn tst(clt: &mut reqwest::Client, scheme: &'static str, domain: &str) -> Result<()> {
            let req = clt.get(format!("{scheme}://{domain}/v2/_catalog")).build()?;
            let _ = clt.execute(req).await?.text().await?;
            Ok(())
        }

        let mut http_only = vec![];
        for domain in &hosts {
            if tst(&mut clt, "https", domain).await.is_err()
                && tst(&mut clt, "http", domain).await.is_ok()
            {
                http_only.push(*domain);
            }
        }

        let config = buildkitd_config(&self.registry_mirrors, http_only);
        let cfg = if config != buildkitd::Config::default() {
            let config = toml::to_string_pretty(&config)
                .map_err(|e| anyhow!("Cannot serialize {config:?}: {e}"))?;
            let cfg = tmp().join(format!("{:#x}.toml", crc32fast::hash(config.as_bytes())));
            fs().write(&cfg, &config)
                .map_err(|e| anyhow!("Failed writing buildkitd {cfg}: {e}"))?;
            Some(cfg)
        } else {
            None
        };

        let img = if let Some(ref img) = self.builder.image {
            img.clone()
        } else {
            fetch_digest(&self.runner, &BUILDKIT_IMAGE).await?
        };

        let host_network = hosts.iter().any(|domain| is_local(domain));
        self.create_builder_with(container_args(name, cfg.as_deref(), host_network, &img)).await?;

        if let Some(cfg) = cfg {
            let _ = fs()
                .remove_file(&cfg)
                .map_err(|e| anyhow!("Failed cleaning up buildkitd config: {e}"));
        }

        self.builder.image = Some(img);
        Ok(())
    }

    async fn create_builder_with(&self, args: impl IntoIterator<Item: AsRef<OsStr>>) -> Result<()> {
        fn builder_transient_error(stderr: &str) -> Result<anyhow::Error> {
            if stderr.contains("existing instance") && stderr.contains("but no append mode") {
                return Ok(anyhow!("A builder with the same name already exists..."));
            }
            if stderr.contains(" context deadline exceeded ") {
                return Ok(anyhow!("Network connection mishap..."));
            }
            bail!("Failed to create builder: {stderr}")
        }

        // Reused across attempts
        let args: Vec<_> = args.into_iter().collect();
        let mut retrier = Retrier::with_max_attempts(5);
        loop {
            assert!(!self.runner.is_none(), "create_builder() called with Runner::None");
            let mut cmd = self.cmd()?;
            cmd.args(&args);
            let (succeeded, _, stderr) = cmd.exec().await?;
            if succeeded {
                break;
            }
            let stderr = String::from_utf8_lossy(&stderr);
            if retrier.continues() {
                retrier.backoff("build", builder_transient_error(&stderr)?).await;
                continue;
            }
            bail!(
                "Could not create builder after {} attempts. Is {} OK?\n{stderr}",
                retrier.max(),
                self.runner
            )
        }

        Ok(())
    }

    async fn try_removing_builder(&self, name: &str, keep_state: bool) -> Result<()> {
        assert!(!self.runner.is_none(), "try_removing_builder() called with Runner::None");
        let mut cmd = self.cmd()?;
        cmd.args(["buildx", "rm", "--builder", name]);
        if keep_state {
            cmd.arg("--keep-state");
        } else {
            cmd.arg("--force");
        }

        let (succeeded, _, stderr) = cmd.exec().await?;
        if !succeeded {
            let stderr = String::from_utf8_lossy(&stderr);
            if stderr.contains(&format!("no builder {name:?} found")) {
                return Ok(());
            }
            bail!("Failed to remove builder {name:?}: {stderr}")
        }
        Ok(())
    }

    async fn list_builders(&self) -> Result<Vec<BuildxBuilder>> {
        assert!(!self.runner.is_none(), "list_builders() called with Runner::None");
        let mut cmd = self.cmd()?;
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

/// What to do about the builder builds use
#[derive(Debug, PartialEq, Eq)]
enum Setup {
    Use,
    Create,
    Recreate,
}

/// The builder builds should use
struct Wanted<'a> {
    name: &'a str,
    /// Whether we manage it, as opposed to the user
    managed: bool,
    ours: &'a Managed,
    image: Option<&'a ImageUri>,
}

impl Wanted<'_> {
    /// Decides what to do, given the `existing` builder of that name.
    /// Also gives advice to show.
    fn setup(
        &self,
        existing: Option<&BuildxBuilder>,
        pooled: Option<&str>,
    ) -> Result<(Setup, Vec<String>)> {
        let Self { name, managed, ours, image } = *self;
        let Some(existing) = existing else {
            if !managed {
                bail!("{BUILDX_BUILDER}={name} does not exist")
            }
            return Ok((Setup::Create, vec![]));
        };

        let (mut recreate, mut advice) = (false, vec![]);

        if let Some(ref endpoint) = ours.endpoint
            && !existing.uses_endpoint(endpoint)
        {
            if managed {
                recreate = true;
            } else if let Some(var) = pooled {
                advice.push(format!(
                    "Existing {BUILDX_BUILDER}={name:?} is not on ${var}'s daemon: its connections are not pooled"
                ));
            }
        }

        if let Some(img) = image
            && !existing.uses_image(img)
        {
            if !managed {
                bail!(
                    "Existing {BUILDX_BUILDER}={name:?} does not match {CARGOGREEN_BUILDER_IMAGE}={img:?}"
                )
            }
            recreate = true;
        }

        if !existing.uses_version_newer_or_equal_to(&LATEST_BUILDKIT) {
            // Exluding "remote": no way to upgrade version there
            if managed && ours.driver != Driver::Remote {
                recreate = true;
            } else {
                advice.push(format!(
                    "
Existing {BUILDX_BUILDER}={name:?} runs a BuildKit version older than v{latest}
Maybe try to remove and re-create your builder with:
    docker buildx rm {name} --keep-state
then run your cargo command again.
",
                    latest = LATEST_BUILDKIT.as_str(),
                ));
            }
        }

        Ok((if recreate { Setup::Recreate } else { Setup::Use }, advice))
    }
}

impl Managed {
    /// `docker buildx create` arguments for builder `name`, when it is our `remote` builder
    fn remote_args(&self, desired: &str) -> Option<Vec<String>> {
        let Self { name, driver: Driver::Remote, endpoint: Some(endpoint), tls } = self else {
            return None;
        };
        if name != desired {
            return None;
        }
        let mut args = vec!["buildx".to_owned(), "create".to_owned(), "--bootstrap".to_owned()];
        args.extend(["--name".to_owned(), name.to_owned()]);
        args.extend(["--driver".to_owned(), Driver::Remote.to_string()]);
        // Not pooled (the pool would handle TLS): let buildx do it
        if let Some(TlsFiles { ca, cert_key }) = tls {
            let mut opts = format!("cacert={ca}");
            if let Some((cert, key)) = cert_key {
                opts.push_str(&format!(",cert={cert},key={key}"));
            }
            args.extend(["--driver-opt".to_owned(), opts]);
        }
        args.push(endpoint.to_string());
        Some(args)
    }
}

/// `docker buildx create` arguments for a `docker-container` builder
#[must_use]
fn container_args(
    name: &str,
    cfg: Option<&Utf8Path>,
    host_network: bool,
    img: &ImageUri,
) -> Vec<String> {
    let mut args = vec!["buildx".to_owned(), "create".to_owned(), "--bootstrap".to_owned()];
    args.extend(["--name".to_owned(), name.to_owned()]);
    args.extend(["--driver".to_owned(), Driver::DockerContainer.to_string()]);
    if let Some(cfg) = cfg {
        args.extend(["--buildkitd-config".to_owned(), cfg.to_string()]);
    }
    if host_network {
        // From [Insecure Entitlement "network.host" not working](https://github.com/docker/buildx/issues/835)
        args.extend([
            "--driver-opt=network=host".to_owned(),
            "--buildkitd-flags".to_owned(),
            "--allow-insecure-entitlement=network.host".to_owned(),
        ]);
    }
    args.push(format!("--driver-opt=image={}", img.noscheme()));
    args
}

/// BuildKit's configuration: docker.io `mirrors` and registries only reachable over `http://`
#[must_use]
fn buildkitd_config<'a>(
    mirrors: &[String],
    http_only: impl IntoIterator<Item = &'a str>,
) -> buildkitd::Config {
    let mut config = buildkitd::Config::default();
    if !mirrors.is_empty() {
        config.set_registry_mirrors("docker.io", mirrors.to_vec());
    }
    for domain in http_only {
        let http = buildkitd::Registry { http: true, ..Default::default() };
        config.registry.insert(domain.to_owned(), http);
    }
    config
}

/// Whether a registry runs on this machine: its host is `localhost` or a loopback IP
#[must_use]
fn is_local(domain: &str) -> bool {
    let Ok(url) = Url::parse(&format!("http://{domain}")) else { return false };
    match url.host() {
        Some(Host::Domain(domain)) => domain == "localhost",
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod setup {
    use anyhow::Result;
    use url::Url;

    use super::{
        BuilderNode, BuildxBuilder, Driver, DriverOpts, MANAGED, Managed, Setup, Wanted,
        buildkitd_config, container_args, is_local,
    };
    use crate::{buildkitd, image_uri::ImageUri, remote::TlsFiles};

    fn url(addr: &str) -> Url {
        Url::parse(addr).unwrap()
    }

    fn img(uri: &str) -> ImageUri {
        ImageUri::try_new(uri).unwrap()
    }

    fn remote(endpoint: &str, tls: Option<TlsFiles>) -> Managed {
        let (name, driver) = ("supergreen-buildkit-h".to_owned(), Driver::Remote);
        Managed { name, driver, endpoint: Some(url(endpoint)), tls }
    }

    fn existing(endpoint: &str, version: &str, image: Option<&str>) -> BuildxBuilder {
        BuildxBuilder {
            name: "b".to_owned(),
            driver: Driver::DockerContainer,
            nodes: vec![BuilderNode {
                version: Some(version.to_owned()),
                endpoint: Some(endpoint.to_owned()),
                driver_opts: image.map(|i| DriverOpts { image: Some(i.to_owned()) }),
            }],
        }
    }

    const NEW: &str = "v99.0.0";
    const OLD: &str = "v0.1.0";

    fn docker_on(endpoint: &str) -> Managed {
        Managed { endpoint: Some(url(endpoint)), ..Managed::default() }
    }

    fn setup(
        managed: bool,
        ours: &Managed,
        image: Option<&ImageUri>,
        existing: Option<&BuildxBuilder>,
        pooled: Option<&str>,
    ) -> Result<(Setup, Vec<String>)> {
        Wanted { name: "b", managed, ours, image }.setup(existing, pooled)
    }

    #[test]
    fn driver_names() {
        for (driver, name) in [
            (Driver::Docker, "docker"),
            (Driver::DockerContainer, "docker-container"),
            (Driver::Remote, "remote"),
            (Driver::Other("kubernetes".to_owned()), "kubernetes"),
        ] {
            assert_eq!(driver.to_string(), name);
            assert_eq!(name.parse::<Driver>().unwrap(), driver);
            assert_eq!(serde_json::to_string(&driver).unwrap(), format!("{name:?}"));
        }
    }

    #[test]
    fn create_missing_managed() {
        let (chosen, advice) = setup(true, &Managed::default(), None, None, None).unwrap();
        assert_eq!((chosen, advice), (Setup::Create, vec![]));
    }

    #[test]
    fn missing_unmanaged() {
        assert!(setup(false, &Managed::default(), None, None, None).is_err());
    }

    #[test]
    fn use_up_to_date() {
        let ours = docker_on("ssh://h");
        let existing = existing("ssh://h", NEW, None);
        let (chosen, advice) = setup(true, &ours, None, Some(&existing), None).unwrap();
        assert_eq!((chosen, advice.len()), (Setup::Use, 0));
    }

    #[test]
    fn recreate_managed_on_other_endpoint() {
        let ours = docker_on("ssh://h");
        let existing = existing("ssh://g", NEW, None);
        let (chosen, advice) = setup(true, &ours, None, Some(&existing), None).unwrap();
        assert_eq!((chosen, advice.len()), (Setup::Recreate, 0));
    }

    #[test]
    fn unmanaged_on_other_endpoint() {
        let ours = docker_on("unix:///pool.sock");
        let bb = existing("ssh://h", NEW, None);
        assert_eq!(setup(false, &ours, None, Some(&bb), None).unwrap(), (Setup::Use, vec![]));
        let (chosen, advice) = setup(false, &ours, None, Some(&bb), Some(DOCKER_HOST!())).unwrap();
        assert_eq!((chosen, advice.len()), (Setup::Use, 1));
    }

    #[test]
    fn image_mismatch() {
        // Builder images are locked
        let image = img("docker-image://docker.io/moby/buildkit:latest")
            .lock(&format!("sha256:{}", "0".repeat(64)));
        let bb = existing("ssh://h", NEW, Some("docker.io/moby/buildkit:old"));
        let ours = docker_on("ssh://h");
        let (chosen, advice) = setup(true, &ours, Some(&image), Some(&bb), None).unwrap();
        assert_eq!((chosen, advice.len()), (Setup::Recreate, 0));
        assert!(setup(false, &ours, Some(&image), Some(&bb), None).is_err());

        let bb = existing("ssh://h", NEW, Some("docker.io/moby/buildkit:latest"));
        let (chosen, advice) = setup(false, &ours, Some(&image), Some(&bb), None).unwrap();
        assert_eq!((chosen, advice), (Setup::Use, vec![]));
    }

    #[test]
    fn old_versions() {
        let bb = existing("ssh://h", OLD, None);
        let (chosen, advice) = setup(true, &docker_on("ssh://h"), None, Some(&bb), None).unwrap();
        assert_eq!((chosen, advice), (Setup::Recreate, vec![]));

        // Cannot upgrade a remote daemon nor others' builders: advise
        for (managed, ours) in [(true, remote("ssh://h", None)), (false, docker_on("ssh://h"))] {
            let (chosen, advice) = setup(managed, &ours, None, Some(&bb), None).unwrap();
            assert_eq!((chosen, advice.len()), (Setup::Use, 1));
        }
    }

    #[test]
    fn remote_args_plain() {
        let args = remote("ssh://h", None).remote_args("supergreen-buildkit-h").unwrap();
        assert_eq!(
            args.join(" "),
            "buildx create --bootstrap --name supergreen-buildkit-h --driver remote ssh://h"
        );
    }

    #[test]
    fn remote_args_tls() {
        let cert_key = Some(("/c/cert.pem".into(), "/c/key.pem".into()));
        let tls = TlsFiles { ca: "/c/ca.pem".into(), cert_key };
        let args = remote("tcp://h:1234", Some(tls)).remote_args("supergreen-buildkit-h").unwrap();
        assert_eq!(
            args.join(" "),
            "buildx create --bootstrap --name supergreen-buildkit-h --driver remote \
             --driver-opt cacert=/c/ca.pem,cert=/c/cert.pem,key=/c/key.pem \
             tcp://h:1234"
        );
    }

    #[test]
    fn remote_args_only_for_our_remote_builder() {
        assert_eq!(remote("ssh://h", None).remote_args("other"), None);
        assert_eq!(Managed::default().remote_args(MANAGED), None);
    }

    #[test]
    fn container_args_minimal() {
        let image = img("docker-image://docker.io/moby/buildkit:latest");
        assert_eq!(
            container_args("b", None, false, &image).join(" "),
            "buildx create --bootstrap --name b --driver docker-container \
             --driver-opt=image=docker.io/moby/buildkit:latest"
        );
    }

    #[test]
    fn container_args_all() {
        let image = img("docker-image://docker.io/moby/buildkit:latest");
        assert_eq!(
            container_args("b", Some("/tmp/x.toml".into()), true, &image).join(" "),
            "buildx create --bootstrap --name b --driver docker-container \
             --buildkitd-config /tmp/x.toml \
             --driver-opt=network=host \
             --buildkitd-flags --allow-insecure-entitlement=network.host \
             --driver-opt=image=docker.io/moby/buildkit:latest"
        );
    }

    #[test]
    fn buildkitd_configs() {
        assert_eq!(buildkitd_config(&[], []), buildkitd::Config::default());

        let config = buildkitd_config(&["mirror.gcr.io".to_owned()], ["localhost:5000"]);
        assert_eq!(config.registry["docker.io"].mirrors, ["mirror.gcr.io"]);
        assert!(config.registry["localhost:5000"].http);
    }

    #[test]
    fn local_registries() {
        assert!(is_local("localhost:5000"));
        assert!(is_local("127.0.0.1:5000"));
        assert!(is_local("127.0.0.2"));
        assert!(is_local("[::1]:5000"));
        assert!(!is_local("localhost.example.com"));
        assert!(!is_local("127.0.0.1.nip.io"));
        assert!(!is_local("::1"));
        assert!(!is_local("ghcr.io"));
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
    let json_bla = include_str!("../fixtures/buildx_ls_bla.json");
    let builders_bla = parse_builders(json_bla.trim()).unwrap();
    assert_eq!(find_builder("beepboop", &builders_bla), None);
    assert_eq!(
        find_builder("bla", &builders_bla).unwrap(),
        &BuildxBuilder {
            name: "bla".to_owned(),
            driver: Driver::DockerContainer,
            nodes: vec![BuilderNode {
                version: Some("v0.22.0".to_owned()),
                endpoint: Some("unix:///var/run/docker.sock".to_owned()),
                driver_opts: Some(DriverOpts {
                    image: Some("docker.io/moby/buildkit:buildx-stable-1".to_owned()),
                }),
            }],
        }
    );

    let json_default = include_str!("../fixtures/buildx_ls_default.json");
    let builders_default = parse_builders(json_default.trim()).unwrap();
    assert_eq!(
        find_builder("default", &builders_default).unwrap(),
        &BuildxBuilder {
            name: "default".to_owned(),
            driver: Driver::Docker,
            nodes: vec![BuilderNode {
                version: Some("v0.23.2".to_owned()),
                endpoint: Some("default".to_owned()),
                driver_opts: None,
            }],
        }
    );
}

#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(rename_all = "PascalCase")]
struct BuilderNode {
    driver_opts: Option<DriverOpts>,
    endpoint: Option<String>,
    version: Option<String>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
struct DriverOpts {
    /// An ImageUri without `^docker-image://`
    image: Option<String>,
}

/// <https://docs.docker.com/build/builders/drivers/>
#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(rename_all = "PascalCase")]
struct BuildxBuilder {
    name: String,
    driver: Driver,
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

    fn uses_endpoint(&self, endpoint: &Url) -> bool {
        let endpoint = endpoint.as_str();
        self.nodes.iter().all(|BuilderNode { endpoint: ep, .. }| ep.as_deref() == Some(endpoint))
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerInspect {
    id: String,
    mounts: Vec<InspectMount>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectMount {
    source: Utf8PathBuf,
}
