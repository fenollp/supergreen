use std::{
    collections::HashMap,
    env,
    ffi::OsStr,
    fmt,
    fs::{self},
    io::ErrorKind,
    ops::Not,
    process::{ExitStatus, Stdio},
    str::FromStr,
    sync::LazyLock,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexSet;
use log::{debug, info};
use reqwest::Client as ReqwestClient;
use serde::{Deserialize, Serialize};
use tokio::{
    fs::File as TokioFile,
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader as TokioBufReader, Lines},
    join,
    process::Command,
    spawn,
    sync::oneshot::{self, error::TryRecvError},
    task::JoinHandle,
    time::error::Elapsed,
};

use crate::{
    add::ENV_ADD_APT,
    du::lock_from_builder_cache,
    ext::{timeout, CommandExt},
    green::{Green, ENV_SET_ENVS},
    image_uri::ImageUri,
    logging::{crate_type_for_logging, ENV_LOG_PATH},
    md::BuildContext,
    stage::Stage,
    PKG,
};

pub(crate) const ERRCODE: &str = "errcode";
pub(crate) const STDERR: &str = "stderr";
pub(crate) const STDOUT: &str = "stdout";

// Envs from BuildKit/Buildx/Docker/Podman that we read
const BUILDKIT_COLORS: &str = "BUILDKIT_COLORS";
pub(crate) const BUILDKIT_HOST: &str = "BUILDKIT_HOST";
const BUILDKIT_PROGRESS: &str = "BUILDKIT_PROGRESS";
const BUILDKIT_TTY_LOG_LINES: &str = "BUILDKIT_TTY_LOG_LINES";
pub(crate) const BUILDX_BUILDER: &str = "BUILDX_BUILDER";
const BUILDX_CPU_PROFILE: &str = "BUILDX_CPU_PROFILE";
const BUILDX_MEM_PROFILE: &str = "BUILDX_MEM_PROFILE";
pub(crate) const DOCKER_BUILDKIT: &str = "DOCKER_BUILDKIT";
pub(crate) const DOCKER_CONTEXT: &str = "DOCKER_CONTEXT";
const DOCKER_DEFAULT_PLATFORM: &str = "DOCKER_DEFAULT_PLATFORM";
const DOCKER_HIDE_LEGACY_COMMANDS: &str = "DOCKER_HIDE_LEGACY_COMMANDS";
pub(crate) const DOCKER_HOST: &str = "DOCKER_HOST";

/// Read envs used by runner, once.
/// https://docs.docker.com/engine/reference/commandline/cli/#environment-variables
/// https://docs.docker.com/build/building/variables/#build-tool-configuration-variables
pub(crate) fn envs() -> HashMap<String, String> {
    [
        BUILDKIT_COLORS,
        BUILDKIT_HOST,
        BUILDKIT_PROGRESS,
        BUILDKIT_TTY_LOG_LINES,
        "BUILDX_BAKE_GIT_AUTH_HEADER",
        "BUILDX_BAKE_GIT_AUTH_TOKEN",
        "BUILDX_BAKE_GIT_SSH",
        BUILDX_BUILDER,
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
static BUILD_UNALTERING_ENVS: LazyLock<Vec<&OsStr>> = LazyLock::new(|| {
    [
        BUILDKIT_COLORS,
        BUILDKIT_HOST,
        BUILDKIT_PROGRESS,
        BUILDKIT_TTY_LOG_LINES,
        BUILDX_BUILDER,
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
});

/// Strip out flags that don't affect a build's outputs:
static BUILD_UNALTERING_FLAGS: &[&str] = &["--cache-from=", "--cache-to="];

#[derive(Debug, Copy, Clone, Default, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
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

impl Green {
    #[must_use]
    pub(crate) fn cmd(&self) -> Command {
        let mut cmd = Command::new(self.runner.to_string());
        cmd.kill_on_drop(true); // Underlying OS process dies with us
        cmd.stdin(Stdio::null());
        if false {
            cmd.arg("--debug");
        }
        cmd.env_clear(); // Pass all envs explicitly only
        cmd.env(DOCKER_BUILDKIT, "1"); // BuildKit is used by either runner

        if let Some(ref name) = self.builder.name {
            cmd.env(BUILDX_BUILDER, name);
        }

        for (var, val) in &self.runner_envs {
            if [BUILDX_BUILDER, DOCKER_BUILDKIT].contains(&var.as_str()) {
                continue;
            }
            info!("passing through runner setting: ${var}={val:?}");
            cmd.env(var, val);
        }

        cmd
    }
}

#[derive(Debug, Copy, Clone, Default, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Network {
    #[default]
    None,
    Default,
    Host,
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Default => write!(f, "default"),
            Self::Host => write!(f, "host"),
        }
    }
}

impl FromStr for Network {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "none" => Ok(Self::None),
            "default" => Ok(Self::Default),
            "host" => Ok(Self::Host),
            _ => {
                let all: Vec<_> = [Self::None, Self::Default, Self::Host]
                    .into_iter()
                    .map(|x| x.to_string())
                    .collect();
                bail!("Network must be one of {all:?}")
            }
        }
    }
}

impl Green {
    /// Read digest from builder cache, then maybe from default cache.
    /// Goal is to have a completely offline mode by default, after a `cargo green fetch`.
    pub(crate) async fn maybe_lock_image(&self, img: &ImageUri) -> Result<ImageUri> {
        if img.locked() {
            return Ok(img.to_owned());
        }
        let errer = |e| anyhow!("Failed locking {img}: {e}");
        if let Some(locked) = self.maybe_lock_from_builder_cache(img).await.map_err(errer)? {
            return Ok(locked);
        }
        if let Some(locked) = self.maybe_lock_from_image_cache(img).await.map_err(errer)? {
            return Ok(locked);
        }
        Ok(img.to_owned())
    }

    /// Reads from builder build cache if any, and falls back to image cache.
    ///
    /// https://docs.docker.com/reference/cli/docker/buildx/imagetools/inspect/
    /// => docker buildx imagetools inspect --format={{json .Manifest.Digest}} img.noscheme()
    ///   Only fetches remote though, and takes ages compared to fetch_digest!
    /// See [Getting an image's digest fast, within a docker-container builder](https://github.com/docker/buildx/discussions/3363)
    async fn maybe_lock_from_builder_cache(&self, img: &ImageUri) -> Result<Option<ImageUri>> {
        let cached = self.images_in_builder_cache().await?;
        Ok(lock_from_builder_cache(img.noscheme(), cached).map(|digest| img.lock(digest)))
    }

    /// If given an un-pinned image URI, query local image cache for its digest.
    /// Returns the given URI, along with its digest if one was found.
    ///
    /// https://docs.docker.com/dhi/core-concepts/digests/
    async fn maybe_lock_from_image_cache(&self, img: &ImageUri) -> Result<Option<ImageUri>> {
        let mut cmd = self.cmd();
        cmd.arg("inspect").arg("--format={{index .RepoDigests 0}}").arg(img.noscheme());

        let (succeeded, stdout, stderr) = cmd.exec().await?;
        if !succeeded {
            let stderr = String::from_utf8_lossy(&stderr);
            if stderr.contains("No such object") {
                return Ok(None);
            }

            let mut help = "";
            if stderr.contains(" executable file not found in ")
                && self.runner_envs.contains_key(DOCKER_HOST)
            {
                // TODO: find actual solutions to 'executable file not found in $PATH'
                // error during connect: Get "http://docker.example.com/v1.51/containers/docker.io/docker/dockerfile:1/json": exec: "ssh": executable file not found in $PATH
                // error during connect: Get "http://docker.example.com/v1.51/containers/json": command [ssh -o ConnectTimeout=30 -T -- gol docker system dial-stdio] has exited with exit status 127, make sure the URL is valid, and Docker 18.09 or later is installed on the remote host: stderr=bash: line 1: docker: command not found
                help = r#"
Maybe have a look at
  https://stackoverflow.com/a/79474080/1418165
  https://github.com/docker/for-mac/issues/4382#issuecomment-603031242
"#
                .trim();
            }
            bail!("BUG: failed to inspect image cache: {stderr}{help}")
        }

        Ok(String::from_utf8_lossy(&stdout)
            .lines()
            .next()
            .and_then(|line| ImageUri::try_new(format!("docker-image://{line}")).ok())
            // NOTE: `inspect` does not keep tag: host/dir/name@sha256:digest (no :tag@)
            .map(|digested| img.lock(digested.digest())))
    }
}

/// If given an un-pinned image URI, query remote image API for its digest.
pub(crate) async fn fetch_digest(img: &ImageUri) -> Result<ImageUri> {
    if img.locked() {
        return Ok(img.to_owned());
    }

    async fn actual(img: &ImageUri) -> Result<ImageUri> {
        let (path, tag) = img.path_and_tag();
        let (ns, slug) = match Utf8Path::new(path).iter().collect::<Vec<_>>()[..] {
            ["docker.io", ns, slug] => (ns, slug),
            _ => bail!("BUG: unhandled registry {img:?}"),
        };

        let txt = ReqwestClient::builder()
            .connect_timeout(Duration::from_secs(4))
            .build()
            .map_err(|e| anyhow!("HTTP client's config/TLS failed: {e}"))?
            .get(format!("https://registry.hub.docker.com/v2/repositories/{ns}/{slug}/tags/{tag}"))
            .send()
            .await
            .map_err(|e| anyhow!("Failed to reach Docker Hub's registry: {e}"))?
            .text()
            .await
            .map_err(|e| anyhow!("Failed to read response from registry: {e}"))?;

        #[derive(Deserialize)]
        struct RegistryResponse {
            digest: String,
        }
        let RegistryResponse { digest } = serde_json::from_str(&txt)
            // NOTE: library images can take a few days to appear, after a Rust release:
            // Error: Failed to decode response from registry: missing field `digest` at line 1 column 130
            // {"message":"httperror 404: tag '1.89.0-slim' not found","errinfo":{"namespace":"library","repository":"rust","tag":"1.89.0-slim"}}
            .map_err(|e| anyhow!("Failed to decode response from registry: {e}\n{txt}"))?;
        // digest ~ sha256:..

        Ok(img.lock(&digest))
    }

    actual(img).await.map_err(|e| anyhow!("Failed getting digest for {img}: {e}"))
}

#[derive(Debug)]
pub(crate) struct Effects {
    pub(crate) written: Vec<Utf8PathBuf>,
    pub(crate) stdout: Vec<String>,
    pub(crate) stderr: Vec<String>,
}

pub(crate) async fn build_cacheonly(
    green: &Green,
    containerfile: &Utf8Path,
    target: Stage,
) -> Result<()> {
    build(green, containerfile, target, &[].into(), None).await.2.map(|_| ())
}

pub(crate) async fn build_out(
    green: &Green,
    containerfile: &Utf8Path,
    target: Stage,
    contexts: &IndexSet<BuildContext>,
    out_dir: &Utf8Path,
) -> (String, String, Result<Effects>) {
    build(green, containerfile, target, contexts, Some(out_dir)).await
}

async fn build(
    green: &Green,
    containerfile: &Utf8Path,
    target: Stage,
    contexts: &IndexSet<BuildContext>,
    out_dir: Option<&Utf8Path>,
) -> (String, String, Result<Effects>) {
    let rtrn = |e| ("".to_owned(), "".to_owned(), Err(e));

    let mut cmd = green.cmd();
    cmd.arg("build");

    //TODO: (use if set) cmd.env("SOURCE_DATE_EPOCH", "0"); // https://reproducible-builds.org/docs/source-date-epoch
    // https://github.com/moby/buildkit/blob/master/docs/build-repro.md#source_date_epoch
    // Set SOURCE_DATE_EPOCH=$(git log -1 --pretty=%ct) for local code, and
    // set it to crates' birth date, in case it's a $HOME/.cargo/registry/cache/...crate
    // set it to the directory's birth date otherwise (should be a relative path to local files).
    // see https://github.com/moby/buildkit/issues/3009#issuecomment-1721565511
    //=> rewrite written files timestamps to not trip cargo's timekeeping

    // `--repro`
    // From https://github.com/docker-library/official-images/issues/16044
    // $ # "none://" is a filler for the build context arg
    // $ docker buildx build \
    //   --load \
    //   -t gcc:local \
    //   --repro from=gcc@sha256:f97e2719cd5138c932a814ca43f3ca7b33fde866e182e7d76d8391ec0b05091f \
    //   none://
    // ...
    // [amd64] Using SLSA provenance sha256:7ecde97c24ea34e1409caf6e91123690fa62d1465ad08f638ebbd75dd381f08f
    // [amd64] Importing Dockerfile blob embedded in the provenance
    // [amd64] Importing build context https://github.com/docker-library/gcc.git#af458ec8254ef7ca3344f12631e2356b20b4a7f1:13
    // [amd64] Importing build-arg SOURCE_DATE_EPOCH=1690467916
    // [amd64] Importing buildpack-deps:bookworm from docker-image://buildpack-deps:bookworm@sha256:bccdd9ebd8dbbb95d41bb5d9de3f654f8cd03b57d65d090ac330d106c87d7ed
    // ...
    // $ diffoci diff gcc@sha256:f97e2719cd5138c932a814ca43f3ca7b33fde866e182e7d76d8391ec0b05091f gcc:local
    // ...

    if false {
        cmd.arg("--no-cache");
        //NOTE: --no-cache-filter target1,target2 --no-cache-filter=target3 (&&)
        // TODO: 'id~=REGEXP as per https://github.com/containerd/containerd/blob/20fc2cf8ec70c5c02cd2f1bbe431bc19b2c622a3/pkg/filters/parser.go#L36
    }

    //     cmd.arg(format!("--cache-to=type=registry,ref={img},mode=max,compression=zstd,force-compression=true,oci-mediatypes=true"));
    // // [2024-04-09T07:55:39Z DEBUG lib-autocfg-72217d8ded4d7ec7@177912] ✖ ERROR: Cache export is not supported for the docker driver.
    // // [2024-04-09T07:55:39Z DEBUG lib-autocfg-72217d8ded4d7ec7@177912] ✖ Switch to a different driver, or turn on the containerd image store, and try again.
    // // [2024-04-09T07:55:39Z DEBUG lib-autocfg-72217d8ded4d7ec7@177912] ✖ Learn more at https://docs.docker.com/go/build-cache-backends/
    //TODO: experiment --cache-to=type=inline => try ,mode=max
    //ignore-error=true

    if !green.cache_images.is_empty() {
        let maxready = green.builder.has_maxready();
        for img in &green.cache_images {
            let img = img.noscheme();
            cmd.arg(format!(
                "--cache-from=type=registry,ref={img}{mode}",
                mode = if maxready { ",mode=max" } else { "" }
            ));

            if maxready {
                continue;
            }

            // TODO: include enough info for repro
            // => rustc shortcommit, ..?
            // Can buildx give list of all inputs? || short hash(dockerfile + call + envs)
            //TODO: include --target=platform in image tag, per: https://github.com/docker/buildx/discussions/1382
            cmd.arg(format!("--tag={img}:{target}"));

            // TODO? additionally filter for only root package
            assert_eq!('b', crate_type_for_logging("bin").to_ascii_lowercase());
            if target.trim_start_matches(|c| c != '-').starts_with("-b-") {
                cmd.arg(format!("--tag={img}:latest"));
            }
        }
        if !maxready {
            cmd.arg("--build-arg=BUILDKIT_INLINE_CACHE=1"); // https://docs.docker.com/build/cache/backends/inline
            cmd.arg("--load"); //FIXME: this should not be needed
        }
    }

    if false {
        // TODO: https://docs.docker.com/build/attestations/
        cmd.arg("--provenance=mode=max");
        cmd.arg("--sbom=true");
    }
    //cmd.arg("--metadata-file=/tmp/meta.json"); => {"buildx.build.ref": "default/default/o5c4435yz6o6xxxhdvekx5lmn"}

    //TODO? --annotation=(PLATFORM=)KEY=VALUE

    cmd.arg(format!("--network={}", green.image.with_network));

    cmd.arg("--platform=local");
    cmd.arg("--pull=false");
    cmd.arg(format!("--target={target}"));
    if let Some(out_dir) = out_dir {
        cmd.arg(format!("--output=type=local,dest={out_dir}"));
    } else {
        // https://docs.docker.com/build/exporters/#cache-only-export
        cmd.arg("--output=type=cacheonly");
    }
    // cmd.arg("--build-arg=BUILDKIT_MULTI_PLATFORM=1"); // "deterministic output"? adds /linux_amd64/ to extracted cratesio

    // TODO: do without local Docker-compatible CLI
    // https://github.com/pyaillet/doggy
    // https://lib.rs/crates/bollard

    for BuildContext { name, uri } in contexts {
        cmd.arg(format!("--build-context={name}={uri}"));
    }

    cmd.arg("-").stdin(Stdio::piped()); // Pass Dockerfile via STDIN, this way there's no default filesystem context.
    if out_dir.is_some() {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    }

    let call = cmd.show();
    info!("Starting `{envs} {call} <{containerfile}`", envs = cmd.envs_string(&[]));
    let call = call
        .split_whitespace()
        .filter(|flag| !BUILD_UNALTERING_FLAGS.iter().any(|prefix| flag.contains(prefix)))
        .collect::<Vec<_>>()
        .join(" ");
    let envs = cmd.envs_string(&BUILD_UNALTERING_ENVS);

    let (status, effects) = match run_build(cmd, &call, containerfile, target, out_dir).await {
        Ok((status, effects)) => (status, effects),
        Err(e) => return rtrn(e),
    };

    // Something is very wrong here. Try to be helpful by logging some info about runner config:
    if !status.success() {
        let logs =
            env::var(ENV_LOG_PATH).map(|val| format!("\nCheck logs at {val}")).unwrap_or_default();
        return rtrn(anyhow!(
            "Runner failed.{logs}
Please report an issue along with information from the following:
* {runner} buildx version
* {runner} info
* {runner} buildx ls
* cargo green supergreen env
",
            runner = green.runner,
        ));
    }

    (call, envs, Ok(effects))
}

async fn run_build(
    mut cmd: Command,
    call: &str,
    containerfile: &Utf8Path,
    target: Stage,
    out_dir: Option<&Utf8Path>,
) -> Result<(ExitStatus, Effects)> {
    let start = Instant::now();
    let mut child = cmd.spawn().map_err(|e| anyhow!("Failed starting `{call}`: {e}"))?;

    spawn({
        let containerfile = containerfile.to_owned();
        let mut stdin = child.stdin.take().expect("started");
        async move {
            let reader = TokioFile::open(&containerfile)
                .await
                .map_err(|e| anyhow!("Failed opening (RO) {containerfile}: {e}"))?;
            let mut lines = TokioBufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.starts_with("## ") {
                    continue;
                }
                stdin
                    .write_all(line.as_bytes())
                    .await
                    .map_err(|e| anyhow!("Failed piping {containerfile}: {e}"))?;
                let _ = stdin.write_u8(b'\n').await;
            }
            Ok::<_, anyhow::Error>(())
        }
    });

    // ---

    let pid = child.id().unwrap_or_default();
    info!("Started as pid={pid} in {:?}", start.elapsed());

    let (tx_err, mut rx_err) = oneshot::channel();
    let mut tx_err = Some(tx_err);

    let handles = if out_dir.is_some() {
        let dbg_out = spawn({
            let mut lines = TokioBufReader::new(child.stdout.take().expect("started")).lines();
            async move {
                while let Ok(Some(line)) = lines.next_line().await {
                    let line = strip_ansi_escapes(&line);
                    if line.is_empty() {
                        continue;
                    }
                    info!("➤ {line}");
                }
            }
        });

        let dbg_err = spawn({
            let mut lines = TokioBufReader::new(child.stderr.take().expect("started")).lines();
            async move {
                let mut details: Vec<String> = vec![];
                let mut dones = 0;
                let mut cacheds = 0;
                while let Ok(Some(line)) = lines.next_line().await {
                    let line = strip_ansi_escapes(&line);
                    if line.is_empty() {
                        continue;
                    }
                    info!("✖ {line}");

                    // Capture some approximate stats the runner gives us

                    if line.starts_with("ERROR: ") {
                        if let Some(tx_err) = tx_err.take() {
                            let _ = tx_err.send(line.trim_start_matches("ERROR: ").to_owned());
                        }
                    }

                    // Show data transfers (Bytes, maybe also timings?)
                    for (idx, pattern) in line.as_str().match_indices(" transferring ") {
                        let detail = line[(pattern.len() + idx)..].trim_end_matches(" done");
                        details.push(detail.to_owned());
                    }

                    // Count DONEs and CACHEDs
                    if line.contains(" DONE ") {
                        dones += 1;
                    } else if line.ends_with(" CACHED") {
                        cacheds += 1;
                    }
                }
                info!("Terminating task CACHED:{cacheds} DONE:{dones} {details:?}");
            }
        });

        Some((dbg_out, dbg_err))
    } else {
        None
    };

    // NOTE: storing STDOUT+STDERR within output stage,
    //   as `cargo` relies on messages given through STDERR.
    //     Reading stdio as it comes through via the runner's logging is indeed the faster solution,
    //   however, it appears non-deterministic (cargo 1.87). Or maybe it's only due
    //   to the runner clipping log output (see https://stackoverflow.com/a/75632518/1418165
    //   and https://github.com/moby/buildkit/pull/1754/files on how `--builder` may help).
    //   Also, the `rawjson` "progress mode" may be a simpler log-output to rely on. But then, what about `podman`?

    let (secs, res) = {
        let start = Instant::now();
        let res = child.wait().await;
        (start.elapsed(), res)
    };
    let status = res.map_err(|e| anyhow!("Failed calling `{call}`: {e}"))?;
    info!("build ran in {secs:?}: {status}");

    if let Some((dbg_out, dbg_err)) = handles {
        match join!(timeout(dbg_out), timeout(dbg_err)) {
            (Ok(Ok(())), Ok(Ok(()))) => {}
            (e1, e2) => bail!("BUG: STDIO forwarding crashed: {e1:?} | {e2:?}"),
        }
    }
    drop(child);

    let mut effects = Effects { written: vec![], stdout: vec![], stderr: vec![] };
    if let Some(out_dir) = out_dir {
        fn stdio_path(target: &Stage, out_dir: &Utf8Path, stdio: &'static str) -> Utf8PathBuf {
            out_dir.join(format!("{target}-{stdio}"))
        }

        //TODO? write all output to mem first? and then to disk (except stdio files)
        let out_path = stdio_path(&target, out_dir, STDOUT);
        let err_path = stdio_path(&target, out_dir, STDERR);
        let rcd_path = stdio_path(&target, out_dir, ERRCODE);

        // Read errorcode output file first, and check for build error. NOTE:
        // * if call to rustc fails, the file will exist but the build will complete.
        // * if the call doesn't fail, the file isn't created.
        // * if the build fails that's a bug, and no files will be outputed.
        let errcode = match (TokioFile::open(&rcd_path).await, rx_err.try_recv()) {
            (Ok(errcode), _) => {
                let line = TokioBufReader::new(errcode).lines().next_line().await;
                line.ok().flatten().and_then(|x| x.parse::<i8>().ok())
            }
            (_, Ok(e)) => bail!("Runner BUG: {e}"),
            (Err(e), Err(TryRecvError::Closed)) if e.kind() == ErrorKind::NotFound => None,
            (Err(a), Err(b)) => unreachable!("either {a} | {b}"),
        };

        async fn stdio_lines(stdio: &Utf8Path) -> Result<Lines<TokioBufReader<TokioFile>>> {
            TokioFile::open(&stdio)
                .await
                .map_err(|e| anyhow!("Failed reading {stdio}: {e}"))
                .map(|stdio| TokioBufReader::new(stdio).lines())
        }

        let out = fwd(stdio_lines(&out_path).await?, "➤", fwd_stdout);
        let err = fwd(stdio_lines(&err_path).await?, "✖", fwd_stderr);

        match join!(timeout(out), timeout(err)) {
            (
                Ok(Ok(Ok(Accumulated { stdout, .. }))),
                Ok(Ok(Ok(Accumulated { stderr, written, envs, libs, .. }))),
            ) => {
                info!("Suggested {PKG}-specific config: envs:{} libs:{}", envs.len(), libs.len());
                effects.stdout = stdout;
                effects.stderr = stderr;
                if !written.is_empty() {
                    log_written_files_metadata(&written);
                    effects.written = written;
                }
                if let Some(errcode) = errcode {
                    bail!("Runner failed with exit code {errcode}")
                }
            }
            (Ok(Ok(Err(e))), _) | (_, Ok(Ok(Err(e)))) => {
                bail!("BUG: STDIO forwarding crashed: {e}")
            }
            (Ok(Err(e)), _) | (_, Ok(Err(e))) => {
                bail!("BUG: spawning STDIO forwarding crashed: {e}")
            }
            (Err(Elapsed { .. }), _) | (_, Err(Elapsed { .. })) => {
                bail!("BUG: STDIO forwarding got crickets for some time")
            }
        }
        if let Err(e) = fs::remove_file(&out_path) {
            bail!("Failed `rm {out_path}`: {e}")
        }
        if let Err(e) = fs::remove_file(&err_path) {
            bail!("Failed `rm {err_path}`: {e}")
        }
    }

    Ok((status, effects))
}

fn log_written_files_metadata(written: &[Utf8PathBuf]) {
    info!("rustc wrote {} files:", written.len());
    for f in written {
        info!(
            "metadata for {f:?}: {:?}",
            f.metadata().map(|fmd| format!(
                "created:{c:?} accessed:{a:?} modified:{m:?}",
                c = fmd.created(),
                a = fmd.accessed(),
                m = fmd.modified(),
            ))
        );
    }
}

#[inline]
#[must_use]
fn strip_ansi_escapes(line: &str) -> String {
    line.replace("\\u001b[0m", "")
        .replace("\\u001b[1m", "")
        .replace("\\u001b[33m", "")
        .replace("\\u001b[38;5;12m", "")
        .replace("\\u001b[38;5;9m", "")
}

#[must_use]
fn fwd<R>(
    mut stdio: Lines<R>,
    badge: &'static str,
    fwd_std: impl Fn(&str, &mut Accumulated) + Send + 'static,
) -> JoinHandle<Result<Accumulated>>
where
    R: AsyncBufRead + Unpin + Send + 'static,
{
    debug!("Reading {badge} file");
    let start = Instant::now();
    spawn(async move {
        let mut acc = Accumulated::default();
        loop {
            let maybe_line = stdio.next_line().await;
            let line = maybe_line.map_err(|e| anyhow!("Failed during piping of {badge}: {e:?}"))?;
            let Some(line) = line else { break };
            if line.is_empty() {
                continue;
            }

            debug!("{badge} {}", strip_ansi_escapes(&line));

            if let Some(msg) = lift_stdio(&line) {
                fwd_std(msg, &mut acc);
            }

            // //warning: panic message contains an unused formatting placeholder
            // //--> /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/proc-macro2-1.0.36/build.rs:191:17
            // FIXME un-rewrite /index.crates.io-0000000000000000/ in cargo messages
            // => also in .d files
            // cache should be ok (cargo's point of view) if written right after green's build(..) call
        }
        debug!("Task {badge} ran for {:?}", start.elapsed());
        drop(stdio);
        Ok(acc)
    })
}

#[derive(Default)]
struct Accumulated {
    written: Vec<Utf8PathBuf>,
    envs: IndexSet<String>,
    libs: IndexSet<String>,
    stdout: Vec<String>,
    stderr: Vec<String>,
}

fn fwd_stderr(msg: &str, acc: &mut Accumulated) {
    if let Some(file) = artifact_written(msg) {
        acc.written.push(file.into());
        info!("rustc wrote {file}");
    }

    let mut msg = msg.to_owned();

    if let Some(var) = env_not_comptime_defined(&msg) {
        acc.envs.insert(var.to_owned());
        if let Some(new_msg) = suggest_set_envs(var, &msg) {
            info!("suggesting to passthrough missing env with set-envs {var:?}");
            msg = new_msg;
        }
    }

    if let Some(lib) = lib_not_found(&msg) {
        acc.libs.insert(lib.to_owned());
        if let Some(new_msg) = suggest_add(lib, &msg) {
            info!("suggesting to add lib to base image {lib:?}");
            msg = new_msg;
        }
    }

    eprintln!("{msg}");
    acc.stderr.push(msg);
}

fn fwd_stdout(msg: &str, acc: &mut Accumulated) {
    info!("(To cargo's STDOUT): {msg}");
    println!("{msg}");
    acc.stdout.push(msg.to_owned());
}

#[test]
fn stdio_passthrough_from_runner() {
    assert_eq!(lift_stdio("#47 1.714 hi!"), Some("hi!"));
    let lines = [
        r#"#47 1.714 {"$message_type":"artifact","artifact":"/tmp/clis-vixargs_0-1-0/release/deps/libclap_derive-fcea659dae5440c4.so","emit":"link"}"#,
        r#"#47 1.714 {"$message_type":"diagnostic","message":"2 warnings emitted","code":null,"level":"warning","spans":[],"children":[],"rendered":"warning: 2 warnings emitted\n\n"}"#,
        r#"#47 1.714 hi!"#,
    ].into_iter().map(|line| lift_stdio(line));
    assert_eq!(
        lines.collect::<Vec<_>>(),
        vec![
            Some(
                r#"{"$message_type":"artifact","artifact":"/tmp/clis-vixargs_0-1-0/release/deps/libclap_derive-fcea659dae5440c4.so","emit":"link"}"#
            ),
            Some(
                r#"{"$message_type":"diagnostic","message":"2 warnings emitted","code":null,"level":"warning","spans":[],"children":[],"rendered":"warning: 2 warnings emitted\n\n"}"#
            ),
            Some("hi!"),
        ]
    );
}

#[must_use]
fn artifact_written(msg: &str) -> Option<&str> {
    #[derive(Deserialize)]
    struct Wrote<'a> {
        artifact: Option<&'a str>,
    }
    let wrote: Option<Wrote> = serde_json::from_str(msg).ok();
    wrote.and_then(|Wrote { artifact }| artifact)
}

#[must_use]
fn suggest(original: &str, suggestion: &str, msg: &str) -> Option<String> {
    let mut data: serde_json::Value = serde_json::from_str(msg).ok()?;
    let rendered = data.get_mut("rendered")?;
    let txt = rendered.as_str()?;

    // '= ' is an ANSI colors -safe choice of separator
    let existing = txt.split("= ").find(|help| help.contains(original))?;

    let mut to = existing.to_owned();
    to.push_str("= ");
    to.push_str(&existing.replace(original, suggestion));

    *rendered = serde_json::json!(txt.replace(existing, &to));
    serde_json::to_string(&data).ok()
}

// Matches (ANSI colors dropped) '''= note: /usr/bin/ld: cannot find -lpq: No such file or directory'''
#[must_use]
fn lib_not_found(msg: &str) -> Option<&str> {
    if let Some((_, rhs)) = msg.split_once(r#"cannot find -l"#) {
        if let Some((lib, _)) = rhs.split_once(": No such file or directory") {
            return Some(lib);
        }
    }
    None
}

// TODO: cleanup how this suggestion appears
#[must_use]
fn suggest_add(lib: &str, msg: &str) -> Option<String> {
    let original = format!("cannot find -l{lib}: No such file or directory");

    let lib = match lib {
        "z" => "zlib1g-dev".to_owned(),
        _ => format!("lib{lib}-dev"),
    };
    let suggestion = format!(
        r#"{PKG}: add `{lib:?}` to either ${ENV_ADD_APT} (apk, apt-get) or to this crate's or your root crate's [package.metadata.green.add] apt list"#
    );

    suggest(&original, &suggestion, msg)
}

#[test]
fn suggesting_add() {
    let input = roundtrip(
        r#"
{
    "$message_type": "diagnostic",
    "message": "linking with `cc` failed: exit status: 1",
    "code": null,
    "level": "error",
    "spans": [],
    "children": [
        {
            "message": " \"cc\" \"-m64\" \"/tmp/rustc7H5UYy/symbols.o\" \"<17 object files omitted>\" \"-Wl,--as-needed\" \"-Wl,-Bstatic\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libdiffy-5f350840256b90b3.rlib,libnu_ansi_term-ecef502768b2f22a.rlib,liboverload-45aafc635a57cff1.rlib,liburl-036f69e82fdcf501.rlib,libidna-ffbc39c7a5d1e77c.rlib,libunicode_normalization-da6cfe0b315d21ba.rlib,libtinyvec-0e7c6d99fc4ecd2c.rlib,libtinyvec_macros-9126bcde4c5f1615.rlib,libunicode_bidi-23766e683251f25e.rlib,libform_urlencoded-ed0427b193415122.rlib,libpercent_encoding-a9ca6250cc102bd0.rlib,libmatches-27d9c5e1e6de7509.rlib,libdotenvy-9fa159acfce4885d.rlib,libchrono-4d3d56d73bf46ec0.rlib,libiana_time_zone-5ed377f5b3d451ef.rlib,libnum_integer-82fb0132f8b53906.rlib,libnum_traits-2f9bcd2a0c30dcff.rlib,libheck-40dbaec9d09d443f.rlib,libdiesel_table_macro_syntax-9f09f66c20aa386f.rlib,libsyn-abced2e57ae6b47a.rlib,libquote-6740271d31439b66.rlib,libproc_macro2-ae7c4c38eaf4593c.rlib,libunicode_ident-aa12b7412dfc4c29.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libproc_macro-*}.rlib\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libclap_complete-2bc2ccaa5088f99a.rlib,libdiesel_migrations-8a52bbbeef5e01db.rlib,libmigrations_internals-52b495bc29f74724.rlib,libtoml-c89277bcdcbf47a1.rlib,libtoml_edit-5bbd23cd7ed53a4f.rlib,libserde_spanned-333f86fbbb2cb152.rlib,libindexmap-6b2daab44e782afb.rlib,libhashbrown-c275e17bff8ef48e.rlib,libwinnow-5ed65ae865e1214b.rlib,libtoml_datetime-69f07545405ed040.rlib,libdiesel-8a4b75e5cbe6865c.rlib,libitoa-f0986793f4dc0c4b.rlib,libbitflags-e95363e9370d640e.rlib,libbyteorder-62ae682b1bf4015e.rlib,libpq_sys-4db52583e4ef9bff.rlib,libserde_regex-b49f06d76e9ca5eb.rlib,libregex-51a29a962320663c.rlib,libaho_corasick-ec90a4b45e50196f.rlib,libmemchr-35aa984256b2a6dc.rlib,libregex_syntax-f2754ca68026052b.rlib,libclap-6885138d353f613d.rlib,libclap_builder-64fd68ff56e9c3b1.rlib,libstrsim-6185e9224c6e4564.rlib,libanstream-d6e3047ceacd4591.rlib,libanstyle_query-5a9f17d21dd97f4e.rlib,libis_terminal-c352a962fc4f6cab.rlib,librustix-2b2c39f9f9d03b14.rlib,liblinux_raw_sys-f6a722b30bf667ab.rlib,libio_lifetimes-a415eef40056a286.rlib,liblibc-859d6bf52555fe98.rlib,libanstyle-587cffdda4d8c45a.rlib,libcolorchoice-6d2fdffc0ac55bb0.rlib,libanstyle_parse-1e447f4b0544156a.rlib,libutf8parse-811cc7a6fc8bc58b.rlib,libclap_lex-c13afce077d893db.rlib,libbitflags-852662162838ab1a.rlib,libonce_cell-c147a48c1dcf8469.rlib,libserde-fa2e373f0760a32a.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-*,libpanic_unwind-*,libobject-*,libmemchr-*,libaddr2line-*,libgimli-*,librustc_demangle-*,libstd_detect-*,libhashbrown-*,librustc_std_workspace_alloc-*,libminiz_oxide-*,libadler2-*,libunwind-*,libcfg_if-*,liblibc-*,liballoc-*,librustc_std_workspace_core-*,libcore-*,libcompiler_builtins-*}.rlib\" \"-Wl,-Bdynamic\" \"-lpq\" \"-lgcc_s\" \"-lutil\" \"-lrt\" \"-lpthread\" \"-lm\" \"-ldl\" \"-lc\" \"-Wl,--eh-frame-hdr\" \"-Wl,-z,noexecstack\" \"-L\" \"/usr/lib/x86_64-linux-gnu\" \"-L\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib\" \"-o\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/diesel-6a254b742936f321\" \"-Wl,--gc-sections\" \"-pie\" \"-Wl,-z,relro,-z,now\" \"-Wl,-O1\" \"-Wl,--strip-debug\" \"-nodefaultlibs\"",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        },
        {
            "message": "some arguments are omitted. use `--verbose` to show all linker arguments",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        },
        {
            "message": "/usr/bin/ld: cannot find -lpq: No such file or directory\ncollect2: error: ld returned 1 exit status\n",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        }
    ],
    "rendered": "error: linking with `cc` failed: exit status: 1\n  |\n  = note:  \"cc\" \"-m64\" \"/tmp/rustc7H5UYy/symbols.o\" \"<17 object files omitted>\" \"-Wl,--as-needed\" \"-Wl,-Bstatic\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libdiffy-5f350840256b90b3.rlib,libnu_ansi_term-ecef502768b2f22a.rlib,liboverload-45aafc635a57cff1.rlib,liburl-036f69e82fdcf501.rlib,libidna-ffbc39c7a5d1e77c.rlib,libunicode_normalization-da6cfe0b315d21ba.rlib,libtinyvec-0e7c6d99fc4ecd2c.rlib,libtinyvec_macros-9126bcde4c5f1615.rlib,libunicode_bidi-23766e683251f25e.rlib,libform_urlencoded-ed0427b193415122.rlib,libpercent_encoding-a9ca6250cc102bd0.rlib,libmatches-27d9c5e1e6de7509.rlib,libdotenvy-9fa159acfce4885d.rlib,libchrono-4d3d56d73bf46ec0.rlib,libiana_time_zone-5ed377f5b3d451ef.rlib,libnum_integer-82fb0132f8b53906.rlib,libnum_traits-2f9bcd2a0c30dcff.rlib,libheck-40dbaec9d09d443f.rlib,libdiesel_table_macro_syntax-9f09f66c20aa386f.rlib,libsyn-abced2e57ae6b47a.rlib,libquote-6740271d31439b66.rlib,libproc_macro2-ae7c4c38eaf4593c.rlib,libunicode_ident-aa12b7412dfc4c29.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libproc_macro-*}.rlib\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libclap_complete-2bc2ccaa5088f99a.rlib,libdiesel_migrations-8a52bbbeef5e01db.rlib,libmigrations_internals-52b495bc29f74724.rlib,libtoml-c89277bcdcbf47a1.rlib,libtoml_edit-5bbd23cd7ed53a4f.rlib,libserde_spanned-333f86fbbb2cb152.rlib,libindexmap-6b2daab44e782afb.rlib,libhashbrown-c275e17bff8ef48e.rlib,libwinnow-5ed65ae865e1214b.rlib,libtoml_datetime-69f07545405ed040.rlib,libdiesel-8a4b75e5cbe6865c.rlib,libitoa-f0986793f4dc0c4b.rlib,libbitflags-e95363e9370d640e.rlib,libbyteorder-62ae682b1bf4015e.rlib,libpq_sys-4db52583e4ef9bff.rlib,libserde_regex-b49f06d76e9ca5eb.rlib,libregex-51a29a962320663c.rlib,libaho_corasick-ec90a4b45e50196f.rlib,libmemchr-35aa984256b2a6dc.rlib,libregex_syntax-f2754ca68026052b.rlib,libclap-6885138d353f613d.rlib,libclap_builder-64fd68ff56e9c3b1.rlib,libstrsim-6185e9224c6e4564.rlib,libanstream-d6e3047ceacd4591.rlib,libanstyle_query-5a9f17d21dd97f4e.rlib,libis_terminal-c352a962fc4f6cab.rlib,librustix-2b2c39f9f9d03b14.rlib,liblinux_raw_sys-f6a722b30bf667ab.rlib,libio_lifetimes-a415eef40056a286.rlib,liblibc-859d6bf52555fe98.rlib,libanstyle-587cffdda4d8c45a.rlib,libcolorchoice-6d2fdffc0ac55bb0.rlib,libanstyle_parse-1e447f4b0544156a.rlib,libutf8parse-811cc7a6fc8bc58b.rlib,libclap_lex-c13afce077d893db.rlib,libbitflags-852662162838ab1a.rlib,libonce_cell-c147a48c1dcf8469.rlib,libserde-fa2e373f0760a32a.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-*,libpanic_unwind-*,libobject-*,libmemchr-*,libaddr2line-*,libgimli-*,librustc_demangle-*,libstd_detect-*,libhashbrown-*,librustc_std_workspace_alloc-*,libminiz_oxide-*,libadler2-*,libunwind-*,libcfg_if-*,liblibc-*,liballoc-*,librustc_std_workspace_core-*,libcore-*,libcompiler_builtins-*}.rlib\" \"-Wl,-Bdynamic\" \"-lpq\" \"-lgcc_s\" \"-lutil\" \"-lrt\" \"-lpthread\" \"-lm\" \"-ldl\" \"-lc\" \"-Wl,--eh-frame-hdr\" \"-Wl,-z,noexecstack\" \"-L\" \"/usr/lib/x86_64-linux-gnu\" \"-L\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib\" \"-o\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/diesel-6a254b742936f321\" \"-Wl,--gc-sections\" \"-pie\" \"-Wl,-z,relro,-z,now\" \"-Wl,-O1\" \"-Wl,--strip-debug\" \"-nodefaultlibs\"\n  = note: some arguments are omitted. use `--verbose` to show all linker arguments\n  = note: /usr/bin/ld: cannot find -lpq: No such file or directory\n          collect2: error: ld returned 1 exit status\n          \n\n"
}"#,
    );

    let output = roundtrip(
        r#"
{
    "$message_type": "diagnostic",
    "message": "linking with `cc` failed: exit status: 1",
    "code": null,
    "level": "error",
    "spans": [],
    "children": [
        {
            "message": " \"cc\" \"-m64\" \"/tmp/rustc7H5UYy/symbols.o\" \"<17 object files omitted>\" \"-Wl,--as-needed\" \"-Wl,-Bstatic\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libdiffy-5f350840256b90b3.rlib,libnu_ansi_term-ecef502768b2f22a.rlib,liboverload-45aafc635a57cff1.rlib,liburl-036f69e82fdcf501.rlib,libidna-ffbc39c7a5d1e77c.rlib,libunicode_normalization-da6cfe0b315d21ba.rlib,libtinyvec-0e7c6d99fc4ecd2c.rlib,libtinyvec_macros-9126bcde4c5f1615.rlib,libunicode_bidi-23766e683251f25e.rlib,libform_urlencoded-ed0427b193415122.rlib,libpercent_encoding-a9ca6250cc102bd0.rlib,libmatches-27d9c5e1e6de7509.rlib,libdotenvy-9fa159acfce4885d.rlib,libchrono-4d3d56d73bf46ec0.rlib,libiana_time_zone-5ed377f5b3d451ef.rlib,libnum_integer-82fb0132f8b53906.rlib,libnum_traits-2f9bcd2a0c30dcff.rlib,libheck-40dbaec9d09d443f.rlib,libdiesel_table_macro_syntax-9f09f66c20aa386f.rlib,libsyn-abced2e57ae6b47a.rlib,libquote-6740271d31439b66.rlib,libproc_macro2-ae7c4c38eaf4593c.rlib,libunicode_ident-aa12b7412dfc4c29.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libproc_macro-*}.rlib\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libclap_complete-2bc2ccaa5088f99a.rlib,libdiesel_migrations-8a52bbbeef5e01db.rlib,libmigrations_internals-52b495bc29f74724.rlib,libtoml-c89277bcdcbf47a1.rlib,libtoml_edit-5bbd23cd7ed53a4f.rlib,libserde_spanned-333f86fbbb2cb152.rlib,libindexmap-6b2daab44e782afb.rlib,libhashbrown-c275e17bff8ef48e.rlib,libwinnow-5ed65ae865e1214b.rlib,libtoml_datetime-69f07545405ed040.rlib,libdiesel-8a4b75e5cbe6865c.rlib,libitoa-f0986793f4dc0c4b.rlib,libbitflags-e95363e9370d640e.rlib,libbyteorder-62ae682b1bf4015e.rlib,libpq_sys-4db52583e4ef9bff.rlib,libserde_regex-b49f06d76e9ca5eb.rlib,libregex-51a29a962320663c.rlib,libaho_corasick-ec90a4b45e50196f.rlib,libmemchr-35aa984256b2a6dc.rlib,libregex_syntax-f2754ca68026052b.rlib,libclap-6885138d353f613d.rlib,libclap_builder-64fd68ff56e9c3b1.rlib,libstrsim-6185e9224c6e4564.rlib,libanstream-d6e3047ceacd4591.rlib,libanstyle_query-5a9f17d21dd97f4e.rlib,libis_terminal-c352a962fc4f6cab.rlib,librustix-2b2c39f9f9d03b14.rlib,liblinux_raw_sys-f6a722b30bf667ab.rlib,libio_lifetimes-a415eef40056a286.rlib,liblibc-859d6bf52555fe98.rlib,libanstyle-587cffdda4d8c45a.rlib,libcolorchoice-6d2fdffc0ac55bb0.rlib,libanstyle_parse-1e447f4b0544156a.rlib,libutf8parse-811cc7a6fc8bc58b.rlib,libclap_lex-c13afce077d893db.rlib,libbitflags-852662162838ab1a.rlib,libonce_cell-c147a48c1dcf8469.rlib,libserde-fa2e373f0760a32a.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-*,libpanic_unwind-*,libobject-*,libmemchr-*,libaddr2line-*,libgimli-*,librustc_demangle-*,libstd_detect-*,libhashbrown-*,librustc_std_workspace_alloc-*,libminiz_oxide-*,libadler2-*,libunwind-*,libcfg_if-*,liblibc-*,liballoc-*,librustc_std_workspace_core-*,libcore-*,libcompiler_builtins-*}.rlib\" \"-Wl,-Bdynamic\" \"-lpq\" \"-lgcc_s\" \"-lutil\" \"-lrt\" \"-lpthread\" \"-lm\" \"-ldl\" \"-lc\" \"-Wl,--eh-frame-hdr\" \"-Wl,-z,noexecstack\" \"-L\" \"/usr/lib/x86_64-linux-gnu\" \"-L\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib\" \"-o\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/diesel-6a254b742936f321\" \"-Wl,--gc-sections\" \"-pie\" \"-Wl,-z,relro,-z,now\" \"-Wl,-O1\" \"-Wl,--strip-debug\" \"-nodefaultlibs\"",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        },
        {
            "message": "some arguments are omitted. use `--verbose` to show all linker arguments",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        },
        {
            "message": "/usr/bin/ld: cannot find -lpq: No such file or directory\ncollect2: error: ld returned 1 exit status\n",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        }
    ],
    "rendered": "error: linking with `cc` failed: exit status: 1\n  |\n  = note:  \"cc\" \"-m64\" \"/tmp/rustc7H5UYy/symbols.o\" \"<17 object files omitted>\" \"-Wl,--as-needed\" \"-Wl,-Bstatic\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libdiffy-5f350840256b90b3.rlib,libnu_ansi_term-ecef502768b2f22a.rlib,liboverload-45aafc635a57cff1.rlib,liburl-036f69e82fdcf501.rlib,libidna-ffbc39c7a5d1e77c.rlib,libunicode_normalization-da6cfe0b315d21ba.rlib,libtinyvec-0e7c6d99fc4ecd2c.rlib,libtinyvec_macros-9126bcde4c5f1615.rlib,libunicode_bidi-23766e683251f25e.rlib,libform_urlencoded-ed0427b193415122.rlib,libpercent_encoding-a9ca6250cc102bd0.rlib,libmatches-27d9c5e1e6de7509.rlib,libdotenvy-9fa159acfce4885d.rlib,libchrono-4d3d56d73bf46ec0.rlib,libiana_time_zone-5ed377f5b3d451ef.rlib,libnum_integer-82fb0132f8b53906.rlib,libnum_traits-2f9bcd2a0c30dcff.rlib,libheck-40dbaec9d09d443f.rlib,libdiesel_table_macro_syntax-9f09f66c20aa386f.rlib,libsyn-abced2e57ae6b47a.rlib,libquote-6740271d31439b66.rlib,libproc_macro2-ae7c4c38eaf4593c.rlib,libunicode_ident-aa12b7412dfc4c29.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libproc_macro-*}.rlib\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libclap_complete-2bc2ccaa5088f99a.rlib,libdiesel_migrations-8a52bbbeef5e01db.rlib,libmigrations_internals-52b495bc29f74724.rlib,libtoml-c89277bcdcbf47a1.rlib,libtoml_edit-5bbd23cd7ed53a4f.rlib,libserde_spanned-333f86fbbb2cb152.rlib,libindexmap-6b2daab44e782afb.rlib,libhashbrown-c275e17bff8ef48e.rlib,libwinnow-5ed65ae865e1214b.rlib,libtoml_datetime-69f07545405ed040.rlib,libdiesel-8a4b75e5cbe6865c.rlib,libitoa-f0986793f4dc0c4b.rlib,libbitflags-e95363e9370d640e.rlib,libbyteorder-62ae682b1bf4015e.rlib,libpq_sys-4db52583e4ef9bff.rlib,libserde_regex-b49f06d76e9ca5eb.rlib,libregex-51a29a962320663c.rlib,libaho_corasick-ec90a4b45e50196f.rlib,libmemchr-35aa984256b2a6dc.rlib,libregex_syntax-f2754ca68026052b.rlib,libclap-6885138d353f613d.rlib,libclap_builder-64fd68ff56e9c3b1.rlib,libstrsim-6185e9224c6e4564.rlib,libanstream-d6e3047ceacd4591.rlib,libanstyle_query-5a9f17d21dd97f4e.rlib,libis_terminal-c352a962fc4f6cab.rlib,librustix-2b2c39f9f9d03b14.rlib,liblinux_raw_sys-f6a722b30bf667ab.rlib,libio_lifetimes-a415eef40056a286.rlib,liblibc-859d6bf52555fe98.rlib,libanstyle-587cffdda4d8c45a.rlib,libcolorchoice-6d2fdffc0ac55bb0.rlib,libanstyle_parse-1e447f4b0544156a.rlib,libutf8parse-811cc7a6fc8bc58b.rlib,libclap_lex-c13afce077d893db.rlib,libbitflags-852662162838ab1a.rlib,libonce_cell-c147a48c1dcf8469.rlib,libserde-fa2e373f0760a32a.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-*,libpanic_unwind-*,libobject-*,libmemchr-*,libaddr2line-*,libgimli-*,librustc_demangle-*,libstd_detect-*,libhashbrown-*,librustc_std_workspace_alloc-*,libminiz_oxide-*,libadler2-*,libunwind-*,libcfg_if-*,liblibc-*,liballoc-*,librustc_std_workspace_core-*,libcore-*,libcompiler_builtins-*}.rlib\" \"-Wl,-Bdynamic\" \"-lpq\" \"-lgcc_s\" \"-lutil\" \"-lrt\" \"-lpthread\" \"-lm\" \"-ldl\" \"-lc\" \"-Wl,--eh-frame-hdr\" \"-Wl,-z,noexecstack\" \"-L\" \"/usr/lib/x86_64-linux-gnu\" \"-L\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib\" \"-o\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/diesel-6a254b742936f321\" \"-Wl,--gc-sections\" \"-pie\" \"-Wl,-z,relro,-z,now\" \"-Wl,-O1\" \"-Wl,--strip-debug\" \"-nodefaultlibs\"\n  = note: some arguments are omitted. use `--verbose` to show all linker arguments\n  = note: /usr/bin/ld: cannot find -lpq: No such file or directory\n          collect2: error: ld returned 1 exit status\n          \n\n= note: /usr/bin/ld: cargo-green: add `\"libpq-dev\"` to either $CARGOGREEN_ADD_APT (apk, apt-get) or to this crate's or your root crate's [package.metadata.green.add] apt list\n          collect2: error: ld returned 1 exit status\n          \n\n"
}"#,
    );

    assert_eq!(lib_not_found(&input), Some("pq"));

    pretty_assertions::assert_eq!(roundtrip(&suggest_add("pq", &input).unwrap()), output);
}

#[test]
fn suggesting_add_ansi() {
    let input = roundtrip(
        r#"
{
    "$message_type": "diagnostic",
    "message": "linking with `cc` failed: exit status: 1",
    "code": null,
    "level": "error",
    "spans": [],
    "children": [
        {
            "message": " \"cc\" \"-m64\" \"/tmp/rustc7H5UYy/symbols.o\" \"<17 object files omitted>\" \"-Wl,--as-needed\" \"-Wl,-Bstatic\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libdiffy-5f350840256b90b3.rlib,libnu_ansi_term-ecef502768b2f22a.rlib,liboverload-45aafc635a57cff1.rlib,liburl-036f69e82fdcf501.rlib,libidna-ffbc39c7a5d1e77c.rlib,libunicode_normalization-da6cfe0b315d21ba.rlib,libtinyvec-0e7c6d99fc4ecd2c.rlib,libtinyvec_macros-9126bcde4c5f1615.rlib,libunicode_bidi-23766e683251f25e.rlib,libform_urlencoded-ed0427b193415122.rlib,libpercent_encoding-a9ca6250cc102bd0.rlib,libmatches-27d9c5e1e6de7509.rlib,libdotenvy-9fa159acfce4885d.rlib,libchrono-4d3d56d73bf46ec0.rlib,libiana_time_zone-5ed377f5b3d451ef.rlib,libnum_integer-82fb0132f8b53906.rlib,libnum_traits-2f9bcd2a0c30dcff.rlib,libheck-40dbaec9d09d443f.rlib,libdiesel_table_macro_syntax-9f09f66c20aa386f.rlib,libsyn-abced2e57ae6b47a.rlib,libquote-6740271d31439b66.rlib,libproc_macro2-ae7c4c38eaf4593c.rlib,libunicode_ident-aa12b7412dfc4c29.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libproc_macro-*}.rlib\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libclap_complete-2bc2ccaa5088f99a.rlib,libdiesel_migrations-8a52bbbeef5e01db.rlib,libmigrations_internals-52b495bc29f74724.rlib,libtoml-c89277bcdcbf47a1.rlib,libtoml_edit-5bbd23cd7ed53a4f.rlib,libserde_spanned-333f86fbbb2cb152.rlib,libindexmap-6b2daab44e782afb.rlib,libhashbrown-c275e17bff8ef48e.rlib,libwinnow-5ed65ae865e1214b.rlib,libtoml_datetime-69f07545405ed040.rlib,libdiesel-8a4b75e5cbe6865c.rlib,libitoa-f0986793f4dc0c4b.rlib,libbitflags-e95363e9370d640e.rlib,libbyteorder-62ae682b1bf4015e.rlib,libpq_sys-4db52583e4ef9bff.rlib,libserde_regex-b49f06d76e9ca5eb.rlib,libregex-51a29a962320663c.rlib,libaho_corasick-ec90a4b45e50196f.rlib,libmemchr-35aa984256b2a6dc.rlib,libregex_syntax-f2754ca68026052b.rlib,libclap-6885138d353f613d.rlib,libclap_builder-64fd68ff56e9c3b1.rlib,libstrsim-6185e9224c6e4564.rlib,libanstream-d6e3047ceacd4591.rlib,libanstyle_query-5a9f17d21dd97f4e.rlib,libis_terminal-c352a962fc4f6cab.rlib,librustix-2b2c39f9f9d03b14.rlib,liblinux_raw_sys-f6a722b30bf667ab.rlib,libio_lifetimes-a415eef40056a286.rlib,liblibc-859d6bf52555fe98.rlib,libanstyle-587cffdda4d8c45a.rlib,libcolorchoice-6d2fdffc0ac55bb0.rlib,libanstyle_parse-1e447f4b0544156a.rlib,libutf8parse-811cc7a6fc8bc58b.rlib,libclap_lex-c13afce077d893db.rlib,libbitflags-852662162838ab1a.rlib,libonce_cell-c147a48c1dcf8469.rlib,libserde-fa2e373f0760a32a.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-*,libpanic_unwind-*,libobject-*,libmemchr-*,libaddr2line-*,libgimli-*,librustc_demangle-*,libstd_detect-*,libhashbrown-*,librustc_std_workspace_alloc-*,libminiz_oxide-*,libadler2-*,libunwind-*,libcfg_if-*,liblibc-*,liballoc-*,librustc_std_workspace_core-*,libcore-*,libcompiler_builtins-*}.rlib\" \"-Wl,-Bdynamic\" \"-lpq\" \"-lgcc_s\" \"-lutil\" \"-lrt\" \"-lpthread\" \"-lm\" \"-ldl\" \"-lc\" \"-Wl,--eh-frame-hdr\" \"-Wl,-z,noexecstack\" \"-L\" \"/usr/lib/x86_64-linux-gnu\" \"-L\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib\" \"-o\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/diesel-6a254b742936f321\" \"-Wl,--gc-sections\" \"-pie\" \"-Wl,-z,relro,-z,now\" \"-Wl,-O1\" \"-Wl,--strip-debug\" \"-nodefaultlibs\"",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        },
        {
            "message": "some arguments are omitted. use `--verbose` to show all linker arguments",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        },
        {
            "message": "/usr/bin/ld: cannot find -lpq: No such file or directory\ncollect2: error: ld returned 1 exit status\n",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        }
    ],
    "rendered": "\u001b[0m\u001b[1m\u001b[38;5;9merror\u001b[0m\u001b[0m\u001b[1m: linking with `cc` failed: exit status: 1\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m:  \"cc\" \"-m64\" \"/tmp/rustc7H5UYy/symbols.o\" \"<17 object files omitted>\" \"-Wl,--as-needed\" \"-Wl,-Bstatic\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libdiffy-5f350840256b90b3.rlib,libnu_ansi_term-ecef502768b2f22a.rlib,liboverload-45aafc635a57cff1.rlib,liburl-036f69e82fdcf501.rlib,libidna-ffbc39c7a5d1e77c.rlib,libunicode_normalization-da6cfe0b315d21ba.rlib,libtinyvec-0e7c6d99fc4ecd2c.rlib,libtinyvec_macros-9126bcde4c5f1615.rlib,libunicode_bidi-23766e683251f25e.rlib,libform_urlencoded-ed0427b193415122.rlib,libpercent_encoding-a9ca6250cc102bd0.rlib,libmatches-27d9c5e1e6de7509.rlib,libdotenvy-9fa159acfce4885d.rlib,libchrono-4d3d56d73bf46ec0.rlib,libiana_time_zone-5ed377f5b3d451ef.rlib,libnum_integer-82fb0132f8b53906.rlib,libnum_traits-2f9bcd2a0c30dcff.rlib,libheck-40dbaec9d09d443f.rlib,libdiesel_table_macro_syntax-9f09f66c20aa386f.rlib,libsyn-abced2e57ae6b47a.rlib,libquote-6740271d31439b66.rlib,libproc_macro2-ae7c4c38eaf4593c.rlib,libunicode_ident-aa12b7412dfc4c29.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libproc_macro-*}.rlib\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libclap_complete-2bc2ccaa5088f99a.rlib,libdiesel_migrations-8a52bbbeef5e01db.rlib,libmigrations_internals-52b495bc29f74724.rlib,libtoml-c89277bcdcbf47a1.rlib,libtoml_edit-5bbd23cd7ed53a4f.rlib,libserde_spanned-333f86fbbb2cb152.rlib,libindexmap-6b2daab44e782afb.rlib,libhashbrown-c275e17bff8ef48e.rlib,libwinnow-5ed65ae865e1214b.rlib,libtoml_datetime-69f07545405ed040.rlib,libdiesel-8a4b75e5cbe6865c.rlib,libitoa-f0986793f4dc0c4b.rlib,libbitflags-e95363e9370d640e.rlib,libbyteorder-62ae682b1bf4015e.rlib,libpq_sys-4db52583e4ef9bff.rlib,libserde_regex-b49f06d76e9ca5eb.rlib,libregex-51a29a962320663c.rlib,libaho_corasick-ec90a4b45e50196f.rlib,libmemchr-35aa984256b2a6dc.rlib,libregex_syntax-f2754ca68026052b.rlib,libclap-6885138d353f613d.rlib,libclap_builder-64fd68ff56e9c3b1.rlib,libstrsim-6185e9224c6e4564.rlib,libanstream-d6e3047ceacd4591.rlib,libanstyle_query-5a9f17d21dd97f4e.rlib,libis_terminal-c352a962fc4f6cab.rlib,librustix-2b2c39f9f9d03b14.rlib,liblinux_raw_sys-f6a722b30bf667ab.rlib,libio_lifetimes-a415eef40056a286.rlib,liblibc-859d6bf52555fe98.rlib,libanstyle-587cffdda4d8c45a.rlib,libcolorchoice-6d2fdffc0ac55bb0.rlib,libanstyle_parse-1e447f4b0544156a.rlib,libutf8parse-811cc7a6fc8bc58b.rlib,libclap_lex-c13afce077d893db.rlib,libbitflags-852662162838ab1a.rlib,libonce_cell-c147a48c1dcf8469.rlib,libserde-fa2e373f0760a32a.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-*,libpanic_unwind-*,libobject-*,libmemchr-*,libaddr2line-*,libgimli-*,librustc_demangle-*,libstd_detect-*,libhashbrown-*,librustc_std_workspace_alloc-*,libminiz_oxide-*,libadler2-*,libunwind-*,libcfg_if-*,liblibc-*,liballoc-*,librustc_std_workspace_core-*,libcore-*,libcompiler_builtins-*}.rlib\" \"-Wl,-Bdynamic\" \"-lpq\" \"-lgcc_s\" \"-lutil\" \"-lrt\" \"-lpthread\" \"-lm\" \"-ldl\" \"-lc\" \"-Wl,--eh-frame-hdr\" \"-Wl,-z,noexecstack\" \"-L\" \"/usr/lib/x86_64-linux-gnu\" \"-L\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib\" \"-o\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/diesel-6a254b742936f321\" \"-Wl,--gc-sections\" \"-pie\" \"-Wl,-z,relro,-z,now\" \"-Wl,-O1\" \"-Wl,--strip-debug\" \"-nodefaultlibs\"\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m: some arguments are omitted. use `--verbose` to show all linker arguments\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m: /usr/bin/ld: cannot find -lpq: No such file or directory\u001b[0m\n\u001b[0m          collect2: error: ld returned 1 exit status\u001b[0m\n\u001b[0m          \u001b[0m\n\n"
}"#,
    );

    let output = roundtrip(
        r#"
{
    "$message_type": "diagnostic",
    "message": "linking with `cc` failed: exit status: 1",
    "code": null,
    "level": "error",
    "spans": [],
    "children": [
        {
            "message": " \"cc\" \"-m64\" \"/tmp/rustc7H5UYy/symbols.o\" \"<17 object files omitted>\" \"-Wl,--as-needed\" \"-Wl,-Bstatic\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libdiffy-5f350840256b90b3.rlib,libnu_ansi_term-ecef502768b2f22a.rlib,liboverload-45aafc635a57cff1.rlib,liburl-036f69e82fdcf501.rlib,libidna-ffbc39c7a5d1e77c.rlib,libunicode_normalization-da6cfe0b315d21ba.rlib,libtinyvec-0e7c6d99fc4ecd2c.rlib,libtinyvec_macros-9126bcde4c5f1615.rlib,libunicode_bidi-23766e683251f25e.rlib,libform_urlencoded-ed0427b193415122.rlib,libpercent_encoding-a9ca6250cc102bd0.rlib,libmatches-27d9c5e1e6de7509.rlib,libdotenvy-9fa159acfce4885d.rlib,libchrono-4d3d56d73bf46ec0.rlib,libiana_time_zone-5ed377f5b3d451ef.rlib,libnum_integer-82fb0132f8b53906.rlib,libnum_traits-2f9bcd2a0c30dcff.rlib,libheck-40dbaec9d09d443f.rlib,libdiesel_table_macro_syntax-9f09f66c20aa386f.rlib,libsyn-abced2e57ae6b47a.rlib,libquote-6740271d31439b66.rlib,libproc_macro2-ae7c4c38eaf4593c.rlib,libunicode_ident-aa12b7412dfc4c29.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libproc_macro-*}.rlib\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libclap_complete-2bc2ccaa5088f99a.rlib,libdiesel_migrations-8a52bbbeef5e01db.rlib,libmigrations_internals-52b495bc29f74724.rlib,libtoml-c89277bcdcbf47a1.rlib,libtoml_edit-5bbd23cd7ed53a4f.rlib,libserde_spanned-333f86fbbb2cb152.rlib,libindexmap-6b2daab44e782afb.rlib,libhashbrown-c275e17bff8ef48e.rlib,libwinnow-5ed65ae865e1214b.rlib,libtoml_datetime-69f07545405ed040.rlib,libdiesel-8a4b75e5cbe6865c.rlib,libitoa-f0986793f4dc0c4b.rlib,libbitflags-e95363e9370d640e.rlib,libbyteorder-62ae682b1bf4015e.rlib,libpq_sys-4db52583e4ef9bff.rlib,libserde_regex-b49f06d76e9ca5eb.rlib,libregex-51a29a962320663c.rlib,libaho_corasick-ec90a4b45e50196f.rlib,libmemchr-35aa984256b2a6dc.rlib,libregex_syntax-f2754ca68026052b.rlib,libclap-6885138d353f613d.rlib,libclap_builder-64fd68ff56e9c3b1.rlib,libstrsim-6185e9224c6e4564.rlib,libanstream-d6e3047ceacd4591.rlib,libanstyle_query-5a9f17d21dd97f4e.rlib,libis_terminal-c352a962fc4f6cab.rlib,librustix-2b2c39f9f9d03b14.rlib,liblinux_raw_sys-f6a722b30bf667ab.rlib,libio_lifetimes-a415eef40056a286.rlib,liblibc-859d6bf52555fe98.rlib,libanstyle-587cffdda4d8c45a.rlib,libcolorchoice-6d2fdffc0ac55bb0.rlib,libanstyle_parse-1e447f4b0544156a.rlib,libutf8parse-811cc7a6fc8bc58b.rlib,libclap_lex-c13afce077d893db.rlib,libbitflags-852662162838ab1a.rlib,libonce_cell-c147a48c1dcf8469.rlib,libserde-fa2e373f0760a32a.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-*,libpanic_unwind-*,libobject-*,libmemchr-*,libaddr2line-*,libgimli-*,librustc_demangle-*,libstd_detect-*,libhashbrown-*,librustc_std_workspace_alloc-*,libminiz_oxide-*,libadler2-*,libunwind-*,libcfg_if-*,liblibc-*,liballoc-*,librustc_std_workspace_core-*,libcore-*,libcompiler_builtins-*}.rlib\" \"-Wl,-Bdynamic\" \"-lpq\" \"-lgcc_s\" \"-lutil\" \"-lrt\" \"-lpthread\" \"-lm\" \"-ldl\" \"-lc\" \"-Wl,--eh-frame-hdr\" \"-Wl,-z,noexecstack\" \"-L\" \"/usr/lib/x86_64-linux-gnu\" \"-L\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib\" \"-o\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/diesel-6a254b742936f321\" \"-Wl,--gc-sections\" \"-pie\" \"-Wl,-z,relro,-z,now\" \"-Wl,-O1\" \"-Wl,--strip-debug\" \"-nodefaultlibs\"",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        },
        {
            "message": "some arguments are omitted. use `--verbose` to show all linker arguments",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        },
        {
            "message": "/usr/bin/ld: cannot find -lpq: No such file or directory\ncollect2: error: ld returned 1 exit status\n",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        }
    ],
    "rendered": "\u001b[0m\u001b[1m\u001b[38;5;9merror\u001b[0m\u001b[0m\u001b[1m: linking with `cc` failed: exit status: 1\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m:  \"cc\" \"-m64\" \"/tmp/rustc7H5UYy/symbols.o\" \"<17 object files omitted>\" \"-Wl,--as-needed\" \"-Wl,-Bstatic\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libdiffy-5f350840256b90b3.rlib,libnu_ansi_term-ecef502768b2f22a.rlib,liboverload-45aafc635a57cff1.rlib,liburl-036f69e82fdcf501.rlib,libidna-ffbc39c7a5d1e77c.rlib,libunicode_normalization-da6cfe0b315d21ba.rlib,libtinyvec-0e7c6d99fc4ecd2c.rlib,libtinyvec_macros-9126bcde4c5f1615.rlib,libunicode_bidi-23766e683251f25e.rlib,libform_urlencoded-ed0427b193415122.rlib,libpercent_encoding-a9ca6250cc102bd0.rlib,libmatches-27d9c5e1e6de7509.rlib,libdotenvy-9fa159acfce4885d.rlib,libchrono-4d3d56d73bf46ec0.rlib,libiana_time_zone-5ed377f5b3d451ef.rlib,libnum_integer-82fb0132f8b53906.rlib,libnum_traits-2f9bcd2a0c30dcff.rlib,libheck-40dbaec9d09d443f.rlib,libdiesel_table_macro_syntax-9f09f66c20aa386f.rlib,libsyn-abced2e57ae6b47a.rlib,libquote-6740271d31439b66.rlib,libproc_macro2-ae7c4c38eaf4593c.rlib,libunicode_ident-aa12b7412dfc4c29.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libproc_macro-*}.rlib\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libclap_complete-2bc2ccaa5088f99a.rlib,libdiesel_migrations-8a52bbbeef5e01db.rlib,libmigrations_internals-52b495bc29f74724.rlib,libtoml-c89277bcdcbf47a1.rlib,libtoml_edit-5bbd23cd7ed53a4f.rlib,libserde_spanned-333f86fbbb2cb152.rlib,libindexmap-6b2daab44e782afb.rlib,libhashbrown-c275e17bff8ef48e.rlib,libwinnow-5ed65ae865e1214b.rlib,libtoml_datetime-69f07545405ed040.rlib,libdiesel-8a4b75e5cbe6865c.rlib,libitoa-f0986793f4dc0c4b.rlib,libbitflags-e95363e9370d640e.rlib,libbyteorder-62ae682b1bf4015e.rlib,libpq_sys-4db52583e4ef9bff.rlib,libserde_regex-b49f06d76e9ca5eb.rlib,libregex-51a29a962320663c.rlib,libaho_corasick-ec90a4b45e50196f.rlib,libmemchr-35aa984256b2a6dc.rlib,libregex_syntax-f2754ca68026052b.rlib,libclap-6885138d353f613d.rlib,libclap_builder-64fd68ff56e9c3b1.rlib,libstrsim-6185e9224c6e4564.rlib,libanstream-d6e3047ceacd4591.rlib,libanstyle_query-5a9f17d21dd97f4e.rlib,libis_terminal-c352a962fc4f6cab.rlib,librustix-2b2c39f9f9d03b14.rlib,liblinux_raw_sys-f6a722b30bf667ab.rlib,libio_lifetimes-a415eef40056a286.rlib,liblibc-859d6bf52555fe98.rlib,libanstyle-587cffdda4d8c45a.rlib,libcolorchoice-6d2fdffc0ac55bb0.rlib,libanstyle_parse-1e447f4b0544156a.rlib,libutf8parse-811cc7a6fc8bc58b.rlib,libclap_lex-c13afce077d893db.rlib,libbitflags-852662162838ab1a.rlib,libonce_cell-c147a48c1dcf8469.rlib,libserde-fa2e373f0760a32a.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-*,libpanic_unwind-*,libobject-*,libmemchr-*,libaddr2line-*,libgimli-*,librustc_demangle-*,libstd_detect-*,libhashbrown-*,librustc_std_workspace_alloc-*,libminiz_oxide-*,libadler2-*,libunwind-*,libcfg_if-*,liblibc-*,liballoc-*,librustc_std_workspace_core-*,libcore-*,libcompiler_builtins-*}.rlib\" \"-Wl,-Bdynamic\" \"-lpq\" \"-lgcc_s\" \"-lutil\" \"-lrt\" \"-lpthread\" \"-lm\" \"-ldl\" \"-lc\" \"-Wl,--eh-frame-hdr\" \"-Wl,-z,noexecstack\" \"-L\" \"/usr/lib/x86_64-linux-gnu\" \"-L\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib\" \"-o\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/diesel-6a254b742936f321\" \"-Wl,--gc-sections\" \"-pie\" \"-Wl,-z,relro,-z,now\" \"-Wl,-O1\" \"-Wl,--strip-debug\" \"-nodefaultlibs\"\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m: some arguments are omitted. use `--verbose` to show all linker arguments\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m: /usr/bin/ld: cannot find -lpq: No such file or directory\u001b[0m\n\u001b[0m          collect2: error: ld returned 1 exit status\u001b[0m\n\u001b[0m          \u001b[0m\n\n= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m: /usr/bin/ld: cargo-green: add `\"libpq-dev\"` to either $CARGOGREEN_ADD_APT (apk, apt-get) or to this crate's or your root crate's [package.metadata.green.add] apt list\u001b[0m\n\u001b[0m          collect2: error: ld returned 1 exit status\u001b[0m\n\u001b[0m          \u001b[0m\n\n"
}"#,
    );

    assert_eq!(lib_not_found(&input), Some("pq"));

    pretty_assertions::assert_eq!(roundtrip(&suggest_add("pq", &input).unwrap()), output);
}

// Matches (ANSI colors dropped) '''"rendered":"error: environment variable `[^`]+` not defined at compile time'''
#[must_use]
fn env_not_comptime_defined(msg: &str) -> Option<&str> {
    if let Some((_, rhs)) = msg.split_once(r#"environment variable `"#) {
        if let Some((var, _)) = rhs.split_once("` not defined at compile time") {
            return Some(var);
        }
    }
    None
}

#[must_use]
fn suggest_set_envs(var: &str, msg: &str) -> Option<String> {
    let original = format!(r#"use `std::env::var("{var}")` to read the variable at run time"#);
    let suggestion = format!(
        r#"{PKG}: add `"{var}"` to either ${ENV_SET_ENVS} or to this crate's or your root crate's [package.metadata.green] set-envs list"#
    );
    suggest(&original, &suggestion, msg)
}

#[cfg(test)]
fn roundtrip(json: &str) -> String {
    let msg: serde_json::Value = serde_json::from_str(json).unwrap();
    serde_json::to_string_pretty(&msg).unwrap()
}

#[test]
fn suggesting_set_envs() {
    let input = roundtrip(
        r#"
{
    "$message_type": "diagnostic",
    "message": "environment variable `MIME_TYPES_GENERATED_PATH` not defined at compile time",
    "code": null,
    "level": "error",
    "spans": [
        {
            "file_name": "/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/mime_guess-2.0.5/src/impl_bin_search.rs",
            "byte_start": 62,
            "byte_end": 95,
            "line_start": 4,
            "line_end": 4,
            "column_start": 10,
            "column_end": 43,
            "is_primary": true,
            "text": [
                {
                    "text": "include!(env!(\"MIME_TYPES_GENERATED_PATH\"));",
                    "highlight_start": 10,
                    "highlight_end": 43
                }
            ],
            "label": null,
            "suggested_replacement": null,
            "suggestion_applicability": null,
            "expansion": {
                "span": {
                    "file_name": "/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/mime_guess-2.0.5/src/impl_bin_search.rs",
                    "byte_start": 62,
                    "byte_end": 95,
                    "line_start": 4,
                    "line_end": 4,
                    "column_start": 10,
                    "column_end": 43,
                    "is_primary": false,
                    "text": [
                        {
                            "text": "include!(env!(\"MIME_TYPES_GENERATED_PATH\"));",
                            "highlight_start": 10,
                            "highlight_end": 43
                        }
                    ],
                    "label": null,
                    "suggested_replacement": null,
                    "suggestion_applicability": null,
                    "expansion": null
                },
                "macro_decl_name": "env!",
                "def_site_span": {
                    "file_name": "/rustc/4d91de4e48198da2e33413efdcd9cd2cc0c46688/library/core/src/macros/mod.rs",
                    "byte_start": 38805,
                    "byte_end": 38821,
                    "line_start": 1101,
                    "line_end": 1101,
                    "column_start": 5,
                    "column_end": 21,
                    "is_primary": false,
                    "text": [],
                    "label": null,
                    "suggested_replacement": null,
                    "suggestion_applicability": null,
                    "expansion": null
                }
            }
        }
    ],
    "children": [
        {
            "message": "use `std::env::var(\"MIME_TYPES_GENERATED_PATH\")` to read the variable at run time",
            "code": null,
            "level": "help",
            "spans": [],
            "children": [],
            "rendered": null
        }
    ],
    "rendered": "error: environment variable `MIME_TYPES_GENERATED_PATH` not defined at compile time\n --> /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/mime_guess-2.0.5/src/impl_bin_search.rs:4:10\n  |\n4 | include!(env!(\"MIME_TYPES_GENERATED_PATH\"));\n  |          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  |\n  = help: use `std::env::var(\"MIME_TYPES_GENERATED_PATH\")` to read the variable at run time\n  = note: this error originates in the macro `env` (in Nightly builds, run with -Z macro-backtrace for more info)\n\n"
}"#,
    );

    let output = roundtrip(
        r#"
{
    "$message_type": "diagnostic",
    "message": "environment variable `MIME_TYPES_GENERATED_PATH` not defined at compile time",
    "code": null,
    "level": "error",
    "spans": [
        {
            "file_name": "/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/mime_guess-2.0.5/src/impl_bin_search.rs",
            "byte_start": 62,
            "byte_end": 95,
            "line_start": 4,
            "line_end": 4,
            "column_start": 10,
            "column_end": 43,
            "is_primary": true,
            "text": [
                {
                    "text": "include!(env!(\"MIME_TYPES_GENERATED_PATH\"));",
                    "highlight_start": 10,
                    "highlight_end": 43
                }
            ],
            "label": null,
            "suggested_replacement": null,
            "suggestion_applicability": null,
            "expansion": {
                "span": {
                    "file_name": "/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/mime_guess-2.0.5/src/impl_bin_search.rs",
                    "byte_start": 62,
                    "byte_end": 95,
                    "line_start": 4,
                    "line_end": 4,
                    "column_start": 10,
                    "column_end": 43,
                    "is_primary": false,
                    "text": [
                        {
                            "text": "include!(env!(\"MIME_TYPES_GENERATED_PATH\"));",
                            "highlight_start": 10,
                            "highlight_end": 43
                        }
                    ],
                    "label": null,
                    "suggested_replacement": null,
                    "suggestion_applicability": null,
                    "expansion": null
                },
                "macro_decl_name": "env!",
                "def_site_span": {
                    "file_name": "/rustc/4d91de4e48198da2e33413efdcd9cd2cc0c46688/library/core/src/macros/mod.rs",
                    "byte_start": 38805,
                    "byte_end": 38821,
                    "line_start": 1101,
                    "line_end": 1101,
                    "column_start": 5,
                    "column_end": 21,
                    "is_primary": false,
                    "text": [],
                    "label": null,
                    "suggested_replacement": null,
                    "suggestion_applicability": null,
                    "expansion": null
                }
            }
        }
    ],
    "children": [
        {
            "message": "use `std::env::var(\"MIME_TYPES_GENERATED_PATH\")` to read the variable at run time",
            "code": null,
            "level": "help",
            "spans": [],
            "children": [],
            "rendered": null
        }
    ],
    "rendered": "error: environment variable `MIME_TYPES_GENERATED_PATH` not defined at compile time\n --> /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/mime_guess-2.0.5/src/impl_bin_search.rs:4:10\n  |\n4 | include!(env!(\"MIME_TYPES_GENERATED_PATH\"));\n  |          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  |\n  = help: use `std::env::var(\"MIME_TYPES_GENERATED_PATH\")` to read the variable at run time\n  = help: cargo-green: add `\"MIME_TYPES_GENERATED_PATH\"` to either $CARGOGREEN_SET_ENVS or to this crate's or your root crate's [package.metadata.green] set-envs list\n  = note: this error originates in the macro `env` (in Nightly builds, run with -Z macro-backtrace for more info)\n\n"
}"#,
    );

    assert_eq!(env_not_comptime_defined(&input), Some("MIME_TYPES_GENERATED_PATH"));

    pretty_assertions::assert_eq!(
        roundtrip(&suggest_set_envs("MIME_TYPES_GENERATED_PATH", &input).unwrap()),
        output
    );
}

#[test]
fn suggesting_set_envs_ansi() {
    let input = roundtrip(
        r#"
{
    "$message_type": "diagnostic",
    "message": "environment variable `TYPENUM_BUILD_OP` not defined at compile time",
    "code": null,
    "level": "error",
    "spans": [
        {
            "file_name": "/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/typenum-1.12.0/src/lib.rs",
            "byte_start": 2460,
            "byte_end": 2484,
            "line_start": 76,
            "line_end": 76,
            "column_start": 14,
            "column_end": 38,
            "is_primary": true,
            "text": [
                {
                    "text": "    include!(env!(\"TYPENUM_BUILD_OP\"));",
                    "highlight_start": 14,
                    "highlight_end": 38
                }
            ],
            "label": null,
            "suggested_replacement": null,
            "suggestion_applicability": null,
            "expansion": {
                "span": {
                    "file_name": "/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/typenum-1.12.0/src/lib.rs",
                    "byte_start": 2460,
                    "byte_end": 2484,
                    "line_start": 76,
                    "line_end": 76,
                    "column_start": 14,
                    "column_end": 38,
                    "is_primary": false,
                    "text": [
                        {
                            "text": "    include!(env!(\"TYPENUM_BUILD_OP\"));",
                            "highlight_start": 14,
                            "highlight_end": 38
                        }
                    ],
                    "label": null,
                    "suggested_replacement": null,
                    "suggestion_applicability": null,
                    "expansion": null
                },
                "macro_decl_name": "env!",
                "def_site_span":
                {
                    "file_name": "/rustc/05f9846f893b09a1be1fc8560e33fc3c815cfecb/library/core/src/macros/mod.rs",
                    "byte_start": 40546,
                    "byte_end": 40562,
                    "line_start": 1164,
                    "line_end": 1164,
                    "column_start": 5,
                    "column_end": 21,
                    "is_primary": false,
                    "text": [],
                    "label": null,
                    "suggested_replacement": null,
                    "suggestion_applicability": null,
                    "expansion": null
                }
            }
        }
    ],
    "children": [
        {
            "message": "use `std::env::var(\"TYPENUM_BUILD_OP\")` to read the variable at run time",
            "code": null,
            "level": "help",
            "spans": [],
            "children": [],
            "rendered": null
        }
    ],
    "rendered": "\u001b[0m\u001b[1m\u001b[38;5;9merror\u001b[0m\u001b[0m\u001b[1m: environment variable `TYPENUM_BUILD_OP` not defined at compile time\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m--> \u001b[0m\u001b[0m/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/typenum-1.12.0/src/lib.rs:76:14\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\n\u001b[0m\u001b[1m\u001b[38;5;12m76\u001b[0m\u001b[0m \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\u001b[0m \u001b[0m\u001b[0m    include!(env!(\"TYPENUM_BUILD_OP\"));\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\u001b[0m              \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;9m^^^^^^^^^^^^^^^^^^^^^^^^\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mhelp\u001b[0m\u001b[0m: use `std::env::var(\"TYPENUM_BUILD_OP\")` to read the variable at run time\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m: this error originates in the macro `env` (in Nightly builds, run with -Z macro-backtrace for more info)\u001b[0m\n\n"
}"#,
    );

    let output = roundtrip(
        r#"{
    "$message_type": "diagnostic",
    "message": "environment variable `TYPENUM_BUILD_OP` not defined at compile time",
    "code": null,
    "level": "error",
    "spans": [
        {
            "file_name": "/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/typenum-1.12.0/src/lib.rs",
            "byte_start": 2460,
            "byte_end": 2484,
            "line_start": 76,
            "line_end": 76,
            "column_start": 14,
            "column_end": 38,
            "is_primary": true,
            "text": [
                {
                    "text": "    include!(env!(\"TYPENUM_BUILD_OP\"));",
                    "highlight_start": 14,
                    "highlight_end": 38
                }
            ],
            "label": null,
            "suggested_replacement": null,
            "suggestion_applicability": null,
            "expansion": {
                "span": {
                    "file_name": "/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/typenum-1.12.0/src/lib.rs",
                    "byte_start": 2460,
                    "byte_end": 2484,
                    "line_start": 76,
                    "line_end": 76,
                    "column_start": 14,
                    "column_end": 38,
                    "is_primary": false,
                    "text": [
                        {
                            "text": "    include!(env!(\"TYPENUM_BUILD_OP\"));",
                            "highlight_start": 14,
                            "highlight_end": 38
                        }
                    ],
                    "label": null,
                    "suggested_replacement": null,
                    "suggestion_applicability": null,
                    "expansion": null
                },
                "macro_decl_name": "env!",
                "def_site_span": {
                    "file_name": "/rustc/05f9846f893b09a1be1fc8560e33fc3c815cfecb/library/core/src/macros/mod.rs",
                    "byte_start": 40546,
                    "byte_end": 40562,
                    "line_start": 1164,
                    "line_end": 1164,
                    "column_start": 5,
                    "column_end": 21,
                    "is_primary": false,
                    "text": [],
                    "label": null,
                    "suggested_replacement": null,
                    "suggestion_applicability": null,
                    "expansion": null
                }
            }
        }
    ],
    "children": [
        {
            "message": "use `std::env::var(\"TYPENUM_BUILD_OP\")` to read the variable at run time",
            "code": null,
            "level": "help",
            "spans": [],
            "children": [],
            "rendered": null
        }
    ],
    "rendered": "\u001b[0m\u001b[1m\u001b[38;5;9merror\u001b[0m\u001b[0m\u001b[1m: environment variable `TYPENUM_BUILD_OP` not defined at compile time\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m--> \u001b[0m\u001b[0m/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/typenum-1.12.0/src/lib.rs:76:14\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\n\u001b[0m\u001b[1m\u001b[38;5;12m76\u001b[0m\u001b[0m \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\u001b[0m \u001b[0m\u001b[0m    include!(env!(\"TYPENUM_BUILD_OP\"));\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\u001b[0m              \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;9m^^^^^^^^^^^^^^^^^^^^^^^^\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mhelp\u001b[0m\u001b[0m: use `std::env::var(\"TYPENUM_BUILD_OP\")` to read the variable at run time\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mhelp\u001b[0m\u001b[0m: cargo-green: add `\"TYPENUM_BUILD_OP\"` to either $CARGOGREEN_SET_ENVS or to this crate's or your root crate's [package.metadata.green] set-envs list\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m: this error originates in the macro `env` (in Nightly builds, run with -Z macro-backtrace for more info)\u001b[0m\n\n"
}"#,
    );

    assert_eq!(env_not_comptime_defined(&input), Some("TYPENUM_BUILD_OP"));

    pretty_assertions::assert_eq!(
        roundtrip(&suggest_set_envs("TYPENUM_BUILD_OP", &input).unwrap()),
        output
    );
}

#[must_use]
fn lift_stdio(line: &str) -> Option<&str> {
    // Docker builds running shell code usually start like: #47 0.057
    let line = line.trim_start_matches(|c| ['#', '.', ' '].contains(&c) || c.is_ascii_digit());
    let msg = line.trim();
    msg.is_empty().not().then_some(msg)
}

#[test]
fn lifting_doesnt_miss_an_artifact() {
    use log::Level;

    let logs = assertx::setup_logging_test();
    let lines = [
        r#"#10 0.111 + cat ./rustc-toolchain ./rustc-toolchain.toml"#,
        r#"#10 0.113 + true"#,
        r#"#10 0.120 ++ which cargo"#,
        r#"#10 0.123 + env CARGO=/usr/local/cargo/bin/cargo CARGO_CRATE_NAME=bitflags CARGO_MANIFEST_DIR=/home/runner/.cargo/registry/src/index.crates.io-0000000000000000/bitflags-0.4.0 CARGO_MANIFEST_PATH=/home/runner/.cargo/registry/src/index.crates.io-0000000000000000/bitflags-0.4.0/Cargo.toml 'CARGO_PKG_AUTHORS=The Rust Project Developers' 'CARGO_PKG_DESCRIPTION=A macro to generate structures which behave like bitflags.\n' CARGO_PKG_HOMEPAGE=https://github.com/rust-lang/bitflags CARGO_PKG_LICENSE=MIT/Apache-2.0 CARGO_PKG_LICENSE_FILE= CARGO_PKG_NAME=bitflags CARGO_PKG_README=README.md CARGO_PKG_REPOSITORY=https://github.com/rust-lang/bitflags CARGO_PKG_RUST_VERSION= CARGO_PKG_VERSION=0.4.0 CARGO_PKG_VERSION_MAJOR=0 CARGO_PKG_VERSION_MINOR=4 CARGO_PKG_VERSION_PATCH=0 CARGO_PKG_VERSION_PRE= CARGOGREEN=1 rustc --crate-name bitflags --edition 2015 --error-format json --json diagnostic-rendered-ansi,artifacts,future-incompat --crate-type lib --emit dep-info,metadata,link -C opt-level=3 -C embed-bitcode=no --check-cfg 'cfg(docsrs,test)' --check-cfg 'cfg(feature, values("no_std"))' -C metadata=c40fd9a620d26196 -C extra-filename=-e976848f96abbbd4 --out-dir /tmp/clis-dbcc_2-2-1/release/deps -C strip=debuginfo -L dependency=/tmp/clis-dbcc_2-2-1/release/deps --cap-lints warn /home/runner/.cargo/registry/src/index.crates.io-0000000000000000/bitflags-0.4.0/src/lib.rs"#,
        r#"#10 0.124 ++ tee /tmp/clis-dbcc_2-2-1/release/deps/out-e976848f96abbbd4-stdout"#,
        r#"#10 0.125 ++ tee /tmp/clis-dbcc_2-2-1/release/deps/out-e976848f96abbbd4-stderr"#,
        r#"#10 0.258 {"$message_type":"artifact","artifact":"/tmp/clis-dbcc_2-2-1/release/deps/bitflags-e976848f96abbbd4.d","emit":"dep-info"}"#,
        r#"#10 0.259 {"$message_type":"diagnostic","message":"extern crate `std` is private and cannot be re-exported","code":{"code":"E0365","explanation":"Private modules cannot be publicly re-exported. This error indicates that you\nattempted to `pub use` a module that was not itself public.\n\nErroneous code example:\n\n```compile_fail,E0365\nmod foo {\n    pub const X: u32 = 1;\n}\n\npub use foo as foo2;\n\nfn main() {}\n```\n\nThe solution to this problem is to ensure that the module that you are\nre-exporting is itself marked with `pub`:\n\n```\npub mod foo {\n    pub const X: u32 = 1;\n}\n\npub use foo as foo2;\n\nfn main() {}\n```\n\nSee the [Use Declarations][use-declarations] section of the reference for\nmore information on this topic.\n\n[use-declarations]: https://doc.rust-lang.org/reference/items/use-declarations.html\n"},"level":"warning","spans":[{"file_name":"/home/runner/.cargo/registry/src/index.crates.io-0000000000000000/bitflags-0.4.0/src/lib.rs","byte_start":1046,"byte_end":1059,"line_start":25,"line_end":25,"column_start":9,"column_end":22,"is_primary":true,"text":[{"text":"pub use std as __core;","highlight_start":9,"highlight_end":22}],"label":null,"suggested_replacement":null,"suggestion_applicability":null,"expansion":null}],"children":[{"message":"this was previously accepted by the compiler but is being phased out; it will become a hard error in a future release!","code":null,"level":"warning","spans":[],"children":[],"rendered":null},{"message":"for more information, see issue #127909 <https://github.com/rust-lang/rust/issues/127909>","code":null,"level":"note","spans":[],"children":[],"rendered":null},{"message":"`#[warn(pub_use_of_private_extern_crate)]` on by default","code":null,"level":"note","spans":[],"children":[],"rendered":null},{"message":"consider making the `extern crate` item publicly accessible","code":null,"level":"help","spans":[{"file_name":"/home/runner/.cargo/registry/src/index.crates.io-0000000000000000/bitflags-0.4.0/src/lib.rs","byte_start":0,"byte_end":0,"line_start":1,"line_end":1,"column_start":1,"column_end":1,"is_primary":true,"text":[],"label":null,"suggested_replacement":"pub ","suggestion_applicability":"MaybeIncorrect","expansion":null}],"children":[],"rendered":null}],"rendered":"warning[E0365]: extern crate `std` is private and cannot be re-exported\n  --> /home/runner/.cargo/registry/src/index.crates.io-0000000000000000/bitflags-0.4.0/src/lib.rs:25:9\n   |\n25 | pub use std as __core;\n   |         ^^^^^^^^^^^^^\n   |\n   = warning: this was previously accepted by the compiler but is being phased out; it will become a hard error in a future release!\n   = note: for more information, see issue #127909 <https://github.com/rust-lang/rust/issues/127909>\n   = note: `#[warn(pub_use_of_private_extern_crate)]` on by default\n\u001b[38;5;14mhelp: consider making the `extern crate` item publicly accessible\n   |\n1  | \u001b[38;5;10mpub // Copyright 2014 The Rust Project Developers. See the COPYRIGHT\n   | \u001b[38;5;10m+++\n\n"}"#,
        r#"#10 0.262 {"$message_type":"artifact","artifact":"/tmp/clis-dbcc_2-2-1/release/deps/libbitflags-e976848f96abbbd4.rmeta","emit":"metadata"}"#,
        r#"#10 0.276 {"$message_type":"artifact","artifact":"/tmp/clis-dbcc_2-2-1/release/deps/libbitflags-e976848f96abbbd4.rlib","emit":"link"}"#,
        r#"#10 0.276 {"$message_type":"diagnostic","message":"1 warning emitted","code":null,"level":"warning","spans":[],"children":[],"rendered":"warning: 1 warning emitted\n\n"}"#,
        r#"#10 0.276 {"$message_type":"diagnostic","message":"For more information about this error, try `rustc --explain E0365`.","code":null,"level":"failure-note","spans":[],"children":[],"rendered":"For more information about this error, try `rustc --explain E0365`.\n"}"#,
        r#"#10 0.276 {"$message_type":"future_incompat","future_incompat_report":[{"diagnostic":{"$message_type":"diagnostic","message":"extern crate `std` is private and cannot be re-exported","code":{"code":"E0365","explanation":"Private modules cannot be publicly re-exported. This error indicates that you\nattempted to `pub use` a module that was not itself public.\n\nErroneous code example:\n\n```compile_fail,E0365\nmod foo {\n    pub const X: u32 = 1;\n}\n\npub use foo as foo2;\n\nfn main() {}\n```\n\nThe solution to this problem is to ensure that the module that you are\nre-exporting is itself marked with `pub`:\n\n```\npub mod foo {\n    pub const X: u32 = 1;\n}\n\npub use foo as foo2;\n\nfn main() {}\n```\n\nSee the [Use Declarations][use-declarations] section of the reference for\nmore information on this topic.\n\n[use-declarations]: https://doc.rust-lang.org/reference/items/use-declarations.html\n"},"level":"warning","spans":[{"file_name":"/home/runner/.cargo/registry/src/index.crates.io-0000000000000000/bitflags-0.4.0/src/lib.rs","byte_start":1046,"byte_end":1059,"line_start":25,"line_end":25,"column_start":9,"column_end":22,"is_primary":true,"text":[{"text":"pub use std as __core;","highlight_start":9,"highlight_end":22}],"label":null,"suggested_replacement":null,"suggestion_applicability":null,"expansion":null}],"children":[{"message":"this was previously accepted by the compiler but is being phased out; it will become a hard error in a future release!","code":null,"level":"warning","spans":[],"children":[],"rendered":null},{"message":"for more information, see issue #127909 <https://github.com/rust-lang/rust/issues/127909>","code":null,"level":"note","spans":[],"children":[],"rendered":null},{"message":"`#[warn(pub_use_of_private_extern_crate)]` on by default","code":null,"level":"note","spans":[],"children":[],"rendered":null},{"message":"consider making the `extern crate` item publicly accessible","code":null,"level":"help","spans":[{"file_name":"/home/runner/.cargo/registry/src/index.crates.io-0000000000000000/bitflags-0.4.0/src/lib.rs","byte_start":0,"byte_end":0,"line_start":1,"line_end":1,"column_start":1,"column_end":1,"is_primary":true,"text":[],"label":null,"suggested_replacement":"pub ","suggestion_applicability":"MaybeIncorrect","expansion":null}],"children":[],"rendered":null}],"rendered":"warning[E0365]: extern crate `std` is private and cannot be re-exported\n  --> /home/runner/.cargo/registry/src/index.crates.io-0000000000000000/bitflags-0.4.0/src/lib.rs:25:9\n   |\n25 | pub use std as __core;\n   |         ^^^^^^^^^^^^^\n   |\n   = warning: this was previously accepted by the compiler but is being phased out; it will become a hard error in a future release!\n   = note: for more information, see issue #127909 <https://github.com/rust-lang/rust/issues/127909>\n   = note: `#[warn(pub_use_of_private_extern_crate)]` on by default\n\u001b[38;5;14mhelp: consider making the `extern crate` item publicly accessible\n   |\n1  | \u001b[38;5;10mpub // Copyright 2014 The Rust Project Developers. See the COPYRIGHT\n   | \u001b[38;5;10m+++\n\n"}}]}"#,
        r#"#10 DONE 0.3s"#,
        r#"#11 [out-e976848f96abbbd4 1/1] COPY --from=dep-l-bitflags-0.4.0-e976848f96abbbd4 /tmp/clis-dbcc_2-2-1/release/deps/*-e976848f96abbbd4* /"#,
        r#"#11 DONE 0.0s"#,
        r#"#12 exporting to client directory"#,
        r#"#12 copying files 44.70kB done"#,
        r#"#12 DONE 0.0s"#,
    ];
    let mut acc = Accumulated::default();

    for line in lines {
        let Some(msg) = lift_stdio(line) else { continue };
        fwd_stderr(msg, &mut acc);
    }

    assert_eq!(
        acc.written,
        [
            "/tmp/clis-dbcc_2-2-1/release/deps/bitflags-e976848f96abbbd4.d",
            "/tmp/clis-dbcc_2-2-1/release/deps/libbitflags-e976848f96abbbd4.rmeta",
            "/tmp/clis-dbcc_2-2-1/release/deps/libbitflags-e976848f96abbbd4.rlib"
        ]
    );

    // Then fwd_stderr calls artifact_written which returns Some("/tmp/thing")
    assertx::assert_logs_contain_in_order!(logs, Level::Info => "rustc wrote /tmp/clis-dbcc_2-2-1/release/deps/bitflags-e976848f96abbbd4.d");
    assertx::assert_logs_contain_in_order!(logs, Level::Info => "rustc wrote /tmp/clis-dbcc_2-2-1/release/deps/libbitflags-e976848f96abbbd4.rmeta");
    assertx::assert_logs_contain_in_order!(logs, Level::Info => "rustc wrote /tmp/clis-dbcc_2-2-1/release/deps/libbitflags-e976848f96abbbd4.rlib");
}
