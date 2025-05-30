use std::{
    env, fmt,
    fs::{self, OpenOptions},
    io::prelude::*,
    mem,
    process::{Output, Stdio},
    str::FromStr,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexSet;
use log::{debug, info};
use reqwest::Client as ReqwestClient;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader as TokioBufReader, Lines},
    join,
    process::Command,
    spawn,
    task::JoinHandle,
    time::{error::Elapsed, timeout},
};

use crate::{
    add::ENV_ADD_APT,
    ext::ShowCmd,
    green::{Green, ENV_SET_ENVS},
    image_uri::ImageUri,
    logging::{crate_type_for_logging, maybe_log, ENV_LOG_PATH},
    md::BuildContext,
    stage::Stage,
    PKG,
};

pub(crate) const MARK_STDOUT: &str = "::STDOUT:: ";
pub(crate) const MARK_STDERR: &str = "::STDERR:: ";

#[derive(Debug, Copy, Clone, Default, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Runner {
    #[default]
    Docker,
    Podman,
    None,
}

impl Runner {
    pub(crate) fn as_cmd(&self) -> Command {
        self.as_debug_cmd(true)
    }

    pub(crate) fn as_nondbg_cmd(&self) -> Command {
        self.as_debug_cmd(false)
    }

    #[must_use]
    fn as_debug_cmd(&self, debug: bool) -> Command {
        let mut cmd = Command::new(self.to_string());
        cmd.kill_on_drop(true); // Makes sure the underlying OS process dies with us
        cmd.stdin(Stdio::null());
        if debug {
            cmd.arg("--debug");
        }
        // TODO: use env_clear https://docs.rs/tokio/latest/tokio/process/struct.Command.html#method.env_clear => pass all buildkit/docker/moby/podman envs explicitly
        cmd
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
                    .into_iter()
                    .map(|x| x.to_string())
                    .collect();
                bail!("Runner must be one of {all:?}")
            }
        }
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

/// If given an un-pinned image URI, query local image cache for its digest.
/// Returns the given URI, along with its digest if one was found.
#[must_use]
pub(crate) async fn maybe_lock_image(green: &Green, img: &ImageUri) -> ImageUri {
    if !img.locked() {
        if let Some(line) = green
            .runner
            .as_cmd()
            .arg("inspect")
            .arg("--format={{index .RepoDigests 0}}")
            .arg(img.noscheme())
            .output()
            .await
            .ok()
            .and_then(|o| o.status.success().then_some(o))
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|x| x.lines().next().map(ToOwned::to_owned))
        {
            // NOTE: `inspect` does not keep tag: host/dir/name@sha256:digest (no :tag@)
            let digested =
                ImageUri::try_new(format!("docker-image://{line}")).expect("inspect's output");
            return img.lock(digested.digest());
        }
    }
    img.to_owned()
}

pub(crate) async fn fetch_digest(img: &ImageUri) -> Result<ImageUri> {
    if img.locked() {
        return Ok(img.to_owned());
    }
    let (path, tag) = img.path_and_tag();
    let (dir, slug) = match Utf8Path::new(path).iter().collect::<Vec<_>>()[..] {
        ["docker.io", dir, slug] => (dir, slug),
        _ => bail!("BUG: unhandled registry {img:?}"),
    };

    let txt = ReqwestClient::builder()
        .connect_timeout(Duration::from_secs(4))
        .build()
        .map_err(|e| anyhow!("HTTP client's config/TLS failed: {e}"))?
        .get(format!("https://registry.hub.docker.com/v2/repositories/{dir}/{slug}/tags/{tag}"))
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
        .map_err(|e| anyhow!("Failed to decode response from registry: {e}"))?;
    // digest ~ sha256:..

    Ok(img.lock(&digest))
}

#[derive(Debug, Default)]
pub(crate) struct Effects {
    pub(crate) written: Vec<Utf8PathBuf>,
}

pub(crate) async fn build_cacheonly(
    green: &Green,
    dockerfile_path: &Utf8Path,
    target: Stage,
) -> Result<()> {
    build(green, dockerfile_path, target, &[].into(), None).await.map(|_| ())
}

pub(crate) async fn build_out(
    green: &Green,
    dockerfile_path: &Utf8Path,
    target: Stage,
    contexts: &IndexSet<BuildContext>,
    out_dir: &Utf8Path,
) -> Result<Effects> {
    build(green, dockerfile_path, target, contexts, Some(out_dir)).await
}

async fn build(
    green: &Green,
    dockerfile_path: &Utf8Path,
    target: Stage,
    contexts: &IndexSet<BuildContext>,
    out_dir: Option<&Utf8Path>,
) -> Result<Effects> {
    let mut cmd = green.runner.as_cmd();
    cmd.arg("build");

    // Makes sure that the BuildKit builder is used by either runner
    cmd.env("DOCKER_BUILDKIT", "1");

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

    // https://docs.docker.com/engine/reference/commandline/cli/#environment-variables
    for var in [
        "BUILDKIT_PROGRESS",
        "BUILDX_BUILDER", //
        "DOCKER_API_VERSION",
        "DOCKER_CERT_PATH",
        "DOCKER_CONFIG",
        "DOCKER_CONTENT_TRUST",
        "DOCKER_CONTENT_TRUST_SERVER",
        "DOCKER_CONTEXT",
        "DOCKER_DEFAULT_PLATFORM",
        "DOCKER_HIDE_LEGACY_COMMANDS",
        "DOCKER_HOST",
        "DOCKER_TLS",
        "DOCKER_TLS_VERIFY",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
    ] {
        if let Ok(val) = env::var(var) {
            cmd.env(var, val);
        }
    }

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

    if !green.cache_images.is_empty() {
        for img in &green.cache_images {
            let img = img.noscheme();
            let mode = if false { ",mode=max" } else { "" }; // TODO: env? builder call?
            cmd.arg(format!("--cache-from=type=registry,ref={img}{mode}"));

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
        cmd.arg("--build-arg=BUILDKIT_INLINE_CACHE=1"); // https://docs.docker.com/build/cache/backends/inline
        cmd.arg("--load"); //FIXME: this should not be needed
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
    let envs: Vec<_> = cmd
        .as_std()
        .get_envs()
        .map(|(k, v)| format!("{}={:?}", k.to_string_lossy(), v.unwrap_or_default()))
        .collect();
    let envs = envs.join(" ");
    info!("Starting {call} (env: {envs}) with {dockerfile_path}");

    let start = Instant::now();
    let mut child = cmd.spawn().map_err(|e| anyhow!("Failed starting {call}: {e}"))?;

    spawn({
        let dockerfile_path = dockerfile_path.to_owned();
        let mut stdin = child.stdin.take().expect("started");
        async move {
            // TODO: buffered IO
            let contents = fs::read_to_string(&dockerfile_path).expect("Piping Dockerfile");
            stdin.write_all(contents.as_bytes()).await.expect("Writing to STDIN");
        }
    });

    // ---

    let pid = child.id().unwrap_or_default();
    info!("Started as pid={pid} in {:?}", start.elapsed());

    let handles = if out_dir.is_some() {
        let out = TokioBufReader::new(child.stdout.take().expect("started")).lines();
        let err = TokioBufReader::new(child.stderr.take().expect("started")).lines();

        // TODO: try rawjson progress mode + find podman equivalent?
        // TODO: set these when --builder is ready https://stackoverflow.com/a/75632518/1418165
        let out_task = fwd(out, "stdout", "➤", MARK_STDOUT);
        let err_task = fwd(err, "stderr", "✖", MARK_STDERR);
        Some((out_task, err_task))
    } else {
        None
    };

    let (secs, res) = {
        let start = Instant::now();
        let res = child.wait().await;
        (start.elapsed(), res)
    };
    let status = res.map_err(|e| anyhow!("Failed calling {call}: {e}"))?;
    info!("build ran in {secs:?}: {status}");

    let mut effects = Effects::default();
    if let Some((out_task, err_task)) = handles {
        let longish = Duration::from_secs(2);
        match join!(timeout(longish, out_task), timeout(longish, err_task)) {
            (Ok(Ok(Ok(_))), Ok(Ok(Ok(Accumulated { written, envs: _, libs: _ })))) => {
                if !written.is_empty() {
                    log_written_files_metadata(&written);
                    effects.written = written;
                }
            }
            (Ok(Ok(Err(e))), _) | (_, Ok(Ok(Err(e)))) => {
                bail!("BUG: STDIO forwarding crashed: {e}")
            }
            (Ok(Err(e)), _) | (_, Ok(Err(e))) => {
                bail!("BUG: spawning STDIO forwarding crashed: {e}")
            }
            (Err(Elapsed { .. }), _) | (_, Err(Elapsed { .. })) => {
                bail!("BUG: STDIO forwarding got crickets for {longish:?}")
            }
        }
    }
    drop(child);

    // Something is very wrong here. Try to be helpful by logging some info about runner config:
    if !status.success() {
        if maybe_log().is_some() {
            bail!("Runner failed. Check logs over at {}", env::var(ENV_LOG_PATH).unwrap())
        }

        // TODO: all these
        // * docker buildx version
        // * docker info
        // * docker buildx ls

        let mut cmd = green.runner.as_nondbg_cmd();
        cmd.arg("info");
        let Output { stdout, stderr, status } =
            cmd.output().await.map_err(|e| anyhow!("Failed starting {}: {e}", cmd.show()))?;
        let stdout = String::from_utf8_lossy(&stdout);
        let stderr = String::from_utf8_lossy(&stderr);
        bail!("Runner info: {status} [STDOUT {stdout}] [STDERR {stderr}]")
    }

    // NOTE: using $CARGO_PRIMARY_PACKAGE still makes >1 hits in rustc calls history: lib + bin, at least.
    if let Some((_, path)) = env::var("CARGO_PRIMARY_PACKAGE").ok().zip(green.final_path.as_deref())
    {
        info!("Writing final Dockerfile to {path}");

        //TODO: use an atomic mv

        let _ = fs::copy(dockerfile_path, path)?;

        let mut file = OpenOptions::new().append(true).open(path)?;
        writeln!(file)?;
        write!(file, "# Pipe this file to")?;
        if !contexts.is_empty() {
            //TODO: or additional-build-arguments
            write!(file, " (not portable due to usage of local build contexts)")?;
        }
        writeln!(file, ":\n# {envs} \\")?;
        let call = &call[1..(call.len() - 1)]; // drops decorative backticks
        writeln!(file, "#   {call}")?;
    }

    Ok(effects)
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
    name: &'static str,
    badge: &'static str,
    mark: &'static str,
) -> JoinHandle<Result<Accumulated>>
where
    R: AsyncBufRead + Unpin + Send + 'static,
{
    debug!("Starting {name} task {badge}");
    let start = Instant::now();
    let fwder = if mark == MARK_STDOUT { fwd_stdout } else { fwd_stderr };
    spawn(async move {
        let mut buf = String::new();
        let mut details: Vec<String> = vec![];
        let mut dones = 0;
        let mut cacheds = 0;
        let mut acc = Accumulated::default();
        let mut first = true;
        loop {
            let maybe_line = stdio.next_line().await;
            if first {
                first = false;
                debug!("Time To First Line for task {name}: {:?}", start.elapsed());
            }
            let line = maybe_line.map_err(|e| anyhow!("Failed during piping of {name}: {e:?}"))?;
            let Some(line) = line else { break };
            if line.is_empty() {
                continue;
            }

            debug!("{badge} {}", strip_ansi_escapes(&line));

            if let Some(msg) = lift_stdio(&line, mark) {
                fwder(msg, &mut buf, &mut acc);
            }

            // //warning: panic message contains an unused formatting placeholder
            // //--> /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/proc-macro2-1.0.36/build.rs:191:17
            // FIXME un-rewrite /index.crates.io-0000000000000000/ in cargo messages
            // => also in .d files
            // cache should be ok (cargo's point of view) if written right after green's build(..) call

            // Show data transfers (Bytes, maybe also timings?)
            const PATTERN: &str = " transferring ";
            for (idx, _pattern) in line.as_str().match_indices(PATTERN) {
                let detail = line[(PATTERN.len() + idx)..].trim_end_matches(" done");
                details.push(detail.to_owned());
            }

            // Count DONEs and CACHEDs
            if line.contains(" DONE ") {
                dones += 1;
            } else if line.ends_with(" CACHED") {
                cacheds += 1;
            }
        }
        debug!("Terminating {name} task CACHED:{cacheds} DONE:{dones} {details:?}");
        drop(stdio);
        Ok(acc)
    })
}

#[derive(Debug, Default)]
struct Accumulated {
    written: Vec<Utf8PathBuf>,
    envs: IndexSet<String>,
    libs: IndexSet<String>,
}

#[test]
fn support_long_broken_json_lines() {
    let logs = assertx::setup_logging_test();
    let lines = [
        r#"#42 1.312 ::STDERR:: {"$message_type":"artifact","artifact":"/tmp/thing","emit":"link""#,
        r#"#42 1.313 ::STDERR:: }"#,
    ];
    let mut buf = String::new();
    let mut acc = Accumulated::default();

    let msg = lift_stdio(lines[0], MARK_STDERR);
    assert_eq!(msg, Some(r#"{"$message_type":"artifact","artifact":"/tmp/thing","emit":"link""#));
    fwd_stderr(msg.unwrap(), &mut buf, &mut acc);
    assert_eq!(buf, r#"{"$message_type":"artifact","artifact":"/tmp/thing","emit":"link""#);
    assert_eq!(acc.written, Vec::<String>::new());

    let msg = lift_stdio(lines[1], MARK_STDERR);
    assert_eq!(msg, Some("}"));
    fwd_stderr(msg.unwrap(), &mut buf, &mut acc);
    assert_eq!(buf, "");
    assert_eq!(acc.written, vec![Utf8PathBuf::from("/tmp/thing")]);

    // Then fwd_stderr
    // calls artifact_written(r#"{"$message_type":"artifact","artifact":"/tmp/thing","emit":"link"}"#)
    // which returns Some("/tmp/thing")
    assertx::assert_logs_contain_in_order!(logs, log::Level::Info => "rustc wrote /tmp/thing");
}

fn fwd_stderr(msg: &str, buf: &mut String, acc: &mut Accumulated) {
    let mut show = |msg: &str| {
        if let Some(file) = artifact_written(msg) {
            acc.written.push(file.into());
            info!("rustc wrote {file}");
        }

        let mut msg = msg.to_owned();

        if let Some(var) = env_not_comptime_defined(&msg) {
            if acc.envs.insert(var.to_owned()) {
                if let Some(new_msg) = suggest_set_envs(var, &msg) {
                    info!("suggesting to passthrough missing env with set-envs {var:?}");
                    msg = new_msg;
                }
            }
        }

        if let Some(lib) = lib_not_found(&msg) {
            if acc.libs.insert(lib.to_owned()) {
                if let Some(new_msg) = suggest_add(lib, &msg) {
                    info!("suggesting to add lib to base image {lib:?}");
                    msg = new_msg;
                }
            }
        }

        info!("(To STDERR for cargo): {msg}");
        eprintln!("{msg}");
    };

    match (buf.is_empty(), msg.starts_with('{'), msg.ends_with('}')) {
        (true, true, true) => show(msg), // json
        (true, true, false) => buf.push_str(msg),
        (true, false, true) => show(msg),  // ?
        (true, false, false) => show(msg), // text
        (false, true, true) => {
            show(&mem::take(buf));
            show(msg) // json
        }
        (false, true, false) => {
            show(&mem::take(buf));
            buf.push_str(msg)
        }
        (false, false, true) => {
            buf.push_str(msg);
            show(&mem::take(buf));
        }
        (false, false, false) => {
            show(&mem::take(buf));
            show(msg) // text
        }
    }
}

fn fwd_stdout(msg: &str, #[expect(clippy::ptr_arg)] _buf: &mut String, _acc: &mut Accumulated) {
    info!("(To cargo's STDOUT): {msg}");
    println!("{msg}");
}

#[test]
fn stdio_passthrough_from_runner() {
    assert_eq!(lift_stdio("#47 1.714 ::STDOUT:: hi!", MARK_STDOUT), Some("hi!"));
    let lines = [
        r#"#47 1.714 ::STDERR:: {"$message_type":"artifact","artifact":"/tmp/clis-vixargs_0-1-0/release/deps/libclap_derive-fcea659dae5440c4.so","emit":"link"}"#,
        r#"#47 1.714 ::STDERR:: {"$message_type":"diagnostic","message":"2 warnings emitted","code":null,"level":"warning","spans":[],"children":[],"rendered":"warning: 2 warnings emitted\n\n"}"#,
        r#"#47 1.714 ::STDOUT:: hi!"#,
    ].into_iter().map(|line| lift_stdio(line, MARK_STDERR));
    assert_eq!(
        lines.collect::<Vec<_>>(),
        vec![
            Some(
                r#"{"$message_type":"artifact","artifact":"/tmp/clis-vixargs_0-1-0/release/deps/libclap_derive-fcea659dae5440c4.so","emit":"link"}"#
            ),
            Some(
                r#"{"$message_type":"diagnostic","message":"2 warnings emitted","code":null,"level":"warning","spans":[],"children":[],"rendered":"warning: 2 warnings emitted\n\n"}"#
            ),
            None,
        ]
    );
}

// TODO? replace with actual JSON deserialization
#[must_use]
fn artifact_written(msg: &str) -> Option<&str> {
    let mut z = msg.split('"');
    let mut a = z.next();
    let mut b = z.next();
    let mut c = z.next();
    loop {
        match (a, b, c) {
            (Some("artifact"), Some(":"), Some(file)) => return Some(file),
            (_, _, Some(_)) => {}
            (_, _, None) => return None,
        }
        (a, b, c) = (b, c, z.next());
    }
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
        r#"add `{lib:?}` to either ${ENV_ADD_APT} (apk, apt-get) or to this crate's or your root crate's [package.metadata.green.add] apt list ({PKG})"#
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
    "rendered": "error: linking with `cc` failed: exit status: 1\n  |\n  = note:  \"cc\" \"-m64\" \"/tmp/rustc7H5UYy/symbols.o\" \"<17 object files omitted>\" \"-Wl,--as-needed\" \"-Wl,-Bstatic\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libdiffy-5f350840256b90b3.rlib,libnu_ansi_term-ecef502768b2f22a.rlib,liboverload-45aafc635a57cff1.rlib,liburl-036f69e82fdcf501.rlib,libidna-ffbc39c7a5d1e77c.rlib,libunicode_normalization-da6cfe0b315d21ba.rlib,libtinyvec-0e7c6d99fc4ecd2c.rlib,libtinyvec_macros-9126bcde4c5f1615.rlib,libunicode_bidi-23766e683251f25e.rlib,libform_urlencoded-ed0427b193415122.rlib,libpercent_encoding-a9ca6250cc102bd0.rlib,libmatches-27d9c5e1e6de7509.rlib,libdotenvy-9fa159acfce4885d.rlib,libchrono-4d3d56d73bf46ec0.rlib,libiana_time_zone-5ed377f5b3d451ef.rlib,libnum_integer-82fb0132f8b53906.rlib,libnum_traits-2f9bcd2a0c30dcff.rlib,libheck-40dbaec9d09d443f.rlib,libdiesel_table_macro_syntax-9f09f66c20aa386f.rlib,libsyn-abced2e57ae6b47a.rlib,libquote-6740271d31439b66.rlib,libproc_macro2-ae7c4c38eaf4593c.rlib,libunicode_ident-aa12b7412dfc4c29.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libproc_macro-*}.rlib\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libclap_complete-2bc2ccaa5088f99a.rlib,libdiesel_migrations-8a52bbbeef5e01db.rlib,libmigrations_internals-52b495bc29f74724.rlib,libtoml-c89277bcdcbf47a1.rlib,libtoml_edit-5bbd23cd7ed53a4f.rlib,libserde_spanned-333f86fbbb2cb152.rlib,libindexmap-6b2daab44e782afb.rlib,libhashbrown-c275e17bff8ef48e.rlib,libwinnow-5ed65ae865e1214b.rlib,libtoml_datetime-69f07545405ed040.rlib,libdiesel-8a4b75e5cbe6865c.rlib,libitoa-f0986793f4dc0c4b.rlib,libbitflags-e95363e9370d640e.rlib,libbyteorder-62ae682b1bf4015e.rlib,libpq_sys-4db52583e4ef9bff.rlib,libserde_regex-b49f06d76e9ca5eb.rlib,libregex-51a29a962320663c.rlib,libaho_corasick-ec90a4b45e50196f.rlib,libmemchr-35aa984256b2a6dc.rlib,libregex_syntax-f2754ca68026052b.rlib,libclap-6885138d353f613d.rlib,libclap_builder-64fd68ff56e9c3b1.rlib,libstrsim-6185e9224c6e4564.rlib,libanstream-d6e3047ceacd4591.rlib,libanstyle_query-5a9f17d21dd97f4e.rlib,libis_terminal-c352a962fc4f6cab.rlib,librustix-2b2c39f9f9d03b14.rlib,liblinux_raw_sys-f6a722b30bf667ab.rlib,libio_lifetimes-a415eef40056a286.rlib,liblibc-859d6bf52555fe98.rlib,libanstyle-587cffdda4d8c45a.rlib,libcolorchoice-6d2fdffc0ac55bb0.rlib,libanstyle_parse-1e447f4b0544156a.rlib,libutf8parse-811cc7a6fc8bc58b.rlib,libclap_lex-c13afce077d893db.rlib,libbitflags-852662162838ab1a.rlib,libonce_cell-c147a48c1dcf8469.rlib,libserde-fa2e373f0760a32a.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-*,libpanic_unwind-*,libobject-*,libmemchr-*,libaddr2line-*,libgimli-*,librustc_demangle-*,libstd_detect-*,libhashbrown-*,librustc_std_workspace_alloc-*,libminiz_oxide-*,libadler2-*,libunwind-*,libcfg_if-*,liblibc-*,liballoc-*,librustc_std_workspace_core-*,libcore-*,libcompiler_builtins-*}.rlib\" \"-Wl,-Bdynamic\" \"-lpq\" \"-lgcc_s\" \"-lutil\" \"-lrt\" \"-lpthread\" \"-lm\" \"-ldl\" \"-lc\" \"-Wl,--eh-frame-hdr\" \"-Wl,-z,noexecstack\" \"-L\" \"/usr/lib/x86_64-linux-gnu\" \"-L\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib\" \"-o\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/diesel-6a254b742936f321\" \"-Wl,--gc-sections\" \"-pie\" \"-Wl,-z,relro,-z,now\" \"-Wl,-O1\" \"-Wl,--strip-debug\" \"-nodefaultlibs\"\n  = note: some arguments are omitted. use `--verbose` to show all linker arguments\n  = note: /usr/bin/ld: cannot find -lpq: No such file or directory\n          collect2: error: ld returned 1 exit status\n          \n\n= note: /usr/bin/ld: add `\"libpq-dev\"` to either $CARGOGREEN_ADD_APT (apk, apt-get) or to this crate's or your root crate's [package.metadata.green.add] apt list (cargo-green)\n          collect2: error: ld returned 1 exit status\n          \n\n"
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
    "rendered": "\u001b[0m\u001b[1m\u001b[38;5;9merror\u001b[0m\u001b[0m\u001b[1m: linking with `cc` failed: exit status: 1\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m:  \"cc\" \"-m64\" \"/tmp/rustc7H5UYy/symbols.o\" \"<17 object files omitted>\" \"-Wl,--as-needed\" \"-Wl,-Bstatic\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libdiffy-5f350840256b90b3.rlib,libnu_ansi_term-ecef502768b2f22a.rlib,liboverload-45aafc635a57cff1.rlib,liburl-036f69e82fdcf501.rlib,libidna-ffbc39c7a5d1e77c.rlib,libunicode_normalization-da6cfe0b315d21ba.rlib,libtinyvec-0e7c6d99fc4ecd2c.rlib,libtinyvec_macros-9126bcde4c5f1615.rlib,libunicode_bidi-23766e683251f25e.rlib,libform_urlencoded-ed0427b193415122.rlib,libpercent_encoding-a9ca6250cc102bd0.rlib,libmatches-27d9c5e1e6de7509.rlib,libdotenvy-9fa159acfce4885d.rlib,libchrono-4d3d56d73bf46ec0.rlib,libiana_time_zone-5ed377f5b3d451ef.rlib,libnum_integer-82fb0132f8b53906.rlib,libnum_traits-2f9bcd2a0c30dcff.rlib,libheck-40dbaec9d09d443f.rlib,libdiesel_table_macro_syntax-9f09f66c20aa386f.rlib,libsyn-abced2e57ae6b47a.rlib,libquote-6740271d31439b66.rlib,libproc_macro2-ae7c4c38eaf4593c.rlib,libunicode_ident-aa12b7412dfc4c29.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libproc_macro-*}.rlib\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libclap_complete-2bc2ccaa5088f99a.rlib,libdiesel_migrations-8a52bbbeef5e01db.rlib,libmigrations_internals-52b495bc29f74724.rlib,libtoml-c89277bcdcbf47a1.rlib,libtoml_edit-5bbd23cd7ed53a4f.rlib,libserde_spanned-333f86fbbb2cb152.rlib,libindexmap-6b2daab44e782afb.rlib,libhashbrown-c275e17bff8ef48e.rlib,libwinnow-5ed65ae865e1214b.rlib,libtoml_datetime-69f07545405ed040.rlib,libdiesel-8a4b75e5cbe6865c.rlib,libitoa-f0986793f4dc0c4b.rlib,libbitflags-e95363e9370d640e.rlib,libbyteorder-62ae682b1bf4015e.rlib,libpq_sys-4db52583e4ef9bff.rlib,libserde_regex-b49f06d76e9ca5eb.rlib,libregex-51a29a962320663c.rlib,libaho_corasick-ec90a4b45e50196f.rlib,libmemchr-35aa984256b2a6dc.rlib,libregex_syntax-f2754ca68026052b.rlib,libclap-6885138d353f613d.rlib,libclap_builder-64fd68ff56e9c3b1.rlib,libstrsim-6185e9224c6e4564.rlib,libanstream-d6e3047ceacd4591.rlib,libanstyle_query-5a9f17d21dd97f4e.rlib,libis_terminal-c352a962fc4f6cab.rlib,librustix-2b2c39f9f9d03b14.rlib,liblinux_raw_sys-f6a722b30bf667ab.rlib,libio_lifetimes-a415eef40056a286.rlib,liblibc-859d6bf52555fe98.rlib,libanstyle-587cffdda4d8c45a.rlib,libcolorchoice-6d2fdffc0ac55bb0.rlib,libanstyle_parse-1e447f4b0544156a.rlib,libutf8parse-811cc7a6fc8bc58b.rlib,libclap_lex-c13afce077d893db.rlib,libbitflags-852662162838ab1a.rlib,libonce_cell-c147a48c1dcf8469.rlib,libserde-fa2e373f0760a32a.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-*,libpanic_unwind-*,libobject-*,libmemchr-*,libaddr2line-*,libgimli-*,librustc_demangle-*,libstd_detect-*,libhashbrown-*,librustc_std_workspace_alloc-*,libminiz_oxide-*,libadler2-*,libunwind-*,libcfg_if-*,liblibc-*,liballoc-*,librustc_std_workspace_core-*,libcore-*,libcompiler_builtins-*}.rlib\" \"-Wl,-Bdynamic\" \"-lpq\" \"-lgcc_s\" \"-lutil\" \"-lrt\" \"-lpthread\" \"-lm\" \"-ldl\" \"-lc\" \"-Wl,--eh-frame-hdr\" \"-Wl,-z,noexecstack\" \"-L\" \"/usr/lib/x86_64-linux-gnu\" \"-L\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib\" \"-o\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/diesel-6a254b742936f321\" \"-Wl,--gc-sections\" \"-pie\" \"-Wl,-z,relro,-z,now\" \"-Wl,-O1\" \"-Wl,--strip-debug\" \"-nodefaultlibs\"\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m: some arguments are omitted. use `--verbose` to show all linker arguments\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m: /usr/bin/ld: cannot find -lpq: No such file or directory\u001b[0m\n\u001b[0m          collect2: error: ld returned 1 exit status\u001b[0m\n\u001b[0m          \u001b[0m\n\n= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m: /usr/bin/ld: add `\"libpq-dev\"` to either $CARGOGREEN_ADD_APT (apk, apt-get) or to this crate's or your root crate's [package.metadata.green.add] apt list (cargo-green)\u001b[0m\n\u001b[0m          collect2: error: ld returned 1 exit status\u001b[0m\n\u001b[0m          \u001b[0m\n\n"
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
        r#"add `"{var}"` to either ${ENV_SET_ENVS} or to this crate's or your root crate's [package.metadata.green] set-envs list ({PKG})"#
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
    "rendered": "error: environment variable `MIME_TYPES_GENERATED_PATH` not defined at compile time\n --> /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/mime_guess-2.0.5/src/impl_bin_search.rs:4:10\n  |\n4 | include!(env!(\"MIME_TYPES_GENERATED_PATH\"));\n  |          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  |\n  = help: use `std::env::var(\"MIME_TYPES_GENERATED_PATH\")` to read the variable at run time\n  = help: add `\"MIME_TYPES_GENERATED_PATH\"` to either $CARGOGREEN_SET_ENVS or to this crate's or your root crate's [package.metadata.green] set-envs list (cargo-green)\n  = note: this error originates in the macro `env` (in Nightly builds, run with -Z macro-backtrace for more info)\n\n"
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
    "rendered": "\u001b[0m\u001b[1m\u001b[38;5;9merror\u001b[0m\u001b[0m\u001b[1m: environment variable `TYPENUM_BUILD_OP` not defined at compile time\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m--> \u001b[0m\u001b[0m/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/typenum-1.12.0/src/lib.rs:76:14\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\n\u001b[0m\u001b[1m\u001b[38;5;12m76\u001b[0m\u001b[0m \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\u001b[0m \u001b[0m\u001b[0m    include!(env!(\"TYPENUM_BUILD_OP\"));\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\u001b[0m              \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;9m^^^^^^^^^^^^^^^^^^^^^^^^\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mhelp\u001b[0m\u001b[0m: use `std::env::var(\"TYPENUM_BUILD_OP\")` to read the variable at run time\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mhelp\u001b[0m\u001b[0m: add `\"TYPENUM_BUILD_OP\"` to either $CARGOGREEN_SET_ENVS or to this crate's or your root crate's [package.metadata.green] set-envs list (cargo-green)\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m: this error originates in the macro `env` (in Nightly builds, run with -Z macro-backtrace for more info)\u001b[0m\n\n"
}"#,
    );

    assert_eq!(env_not_comptime_defined(&input), Some("TYPENUM_BUILD_OP"));

    pretty_assertions::assert_eq!(
        roundtrip(&suggest_set_envs("TYPENUM_BUILD_OP", &input).unwrap()),
        output
    );
}

#[must_use]
fn lift_stdio<'a>(line: &'a str, mark: &'static str) -> Option<&'a str> {
    // Docker builds running shell code usually start like: #47 0.057
    let line = line.trim_start_matches(|c| ['#', '.', ' '].contains(&c) || c.is_ascii_digit());
    let msg = line.trim_start_matches(mark);
    let cut = msg.len() != line.len();
    let msg = msg.trim();
    (cut && !msg.is_empty()).then_some(msg)
}
