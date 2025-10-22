use std::{
    env,
    fs::{self},
    io::ErrorKind,
    ops::Not,
    process::{ExitStatus, Stdio},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexSet;
use log::{debug, info};
use reqwest::Client as ReqwestClient;
use serde::Deserialize;
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
    du::lock_from_builder_cache,
    ext::{timeout, CommandExt},
    green::Green,
    image_uri::ImageUri,
    logging::ENV_LOG_PATH,
    md::BuildContext,
    r#final::is_primary,
    rechrome,
    runner::DOCKER_HOST,
    stage::Stage,
    PKG,
};

pub(crate) const ERRCODE: &str = "errcode";
pub(crate) const STDERR: &str = "stderr";
pub(crate) const STDOUT: &str = "stdout";

impl Green {
    /// Read digest from builder cache, then maybe from default cache.
    ///
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
    /// <https://docs.docker.com/reference/cli/docker/buildx/imagetools/inspect/>
    ///
    /// ```text
    /// docker buildx imagetools inspect --format='{{json .Manifest.Digest}}' img.noscheme()
    /// # Only fetches remote though, and takes ages compared to fetch_digest!
    /// ```
    /// See [Getting an image's digest fast, within a docker-container builder](https://github.com/docker/buildx/discussions/3363)
    async fn maybe_lock_from_builder_cache(&self, img: &ImageUri) -> Result<Option<ImageUri>> {
        let cached = self.images_in_builder_cache().await?;
        Ok(lock_from_builder_cache(img.noscheme(), cached).map(|digest| img.lock(digest)))
    }

    /// If given an un-pinned image URI, query local image cache for its digest.
    ///
    /// Returns the given URI, along with its digest if one was found.
    ///
    /// <https://docs.docker.com/dhi/core-concepts/digests/>
    async fn maybe_lock_from_image_cache(&self, img: &ImageUri) -> Result<Option<ImageUri>> {
        let mut cmd = self.cmd()?;
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

        let domain = "registry.hub.docker.com";
        let (client, req) = ReqwestClient::builder()
            .connect_timeout(Duration::from_secs(4))
            .build()
            .map_err(|e| anyhow!("HTTP client's config/TLS failed: {e}"))?
            .get(format!("https://{domain}/v2/repositories/{ns}/{slug}/tags/{tag}"))
            .build_split();
        let req = req.map_err(|e| anyhow!("Failed to build a request against {domain}: {e}"))?;

        info!("GETing {}", req.url());
        eprintln!("GETing {}", req.url());
        assert!(req.body().is_none());
        assert!(req.headers().is_empty());

        let txt = client
            .execute(req)
            .await
            .map_err(|e| anyhow!("Failed to reach {domain}'s registry: {e}"))?
            .text()
            .await
            .map_err(|e| anyhow!("Failed to read response from {domain} registry: {e}"))?;

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

impl Green {
    pub(crate) async fn build_cacheonly(
        &self,
        containerfile: &Utf8Path,
        target: &Stage,
    ) -> Result<()> {
        self.build(containerfile, target, &[].into(), None).await.2.map(|_| ())
    }

    pub(crate) async fn build_out(
        &self,
        containerfile: &Utf8Path,
        target: &Stage,
        contexts: &IndexSet<BuildContext>,
        out_dir: &Utf8Path,
    ) -> (String, String, Result<Effects>) {
        self.build(containerfile, target, contexts, Some(out_dir)).await
    }

    async fn build(
        &self,
        containerfile: &Utf8Path,
        target: &Stage,
        contexts: &IndexSet<BuildContext>,
        out_dir: Option<&Utf8Path>,
    ) -> (String, String, Result<Effects>) {
        let rtrn = |e| ("".to_owned(), "".to_owned(), Err(e));

        let mut cmd = match self.cmd() {
            Ok(cmd) => cmd,
            Err(e) => return rtrn(e),
        };
        cmd.arg("build");

        //TODO: if allowing additional-build-arguments, deny: --build-arg=BUILDKIT_SYNTAX=

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

        if !self.cache_images.is_empty() {
            let maxready = self.builder.has_maxready();
            for img in &self.cache_images {
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

                if is_primary() {
                    // MAY tag >1 times
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

        cmd.arg(format!("--network={}", self.image.with_network));

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
            .filter(|flag| !self.runner.buildnoop_flags().any(|prefix| flag.contains(prefix)))
            .filter(|flag| !flag.starts_with("--target="))
            .filter(|flag| *flag != "--platform=local")
            .filter(|flag| *flag != "--pull=false")
            .filter(|flag| *flag != "--network=default")
            .map(|flag| if flag.starts_with("--output=") { "--output=." } else { flag })
            .collect::<Vec<_>>()
            .join(" ")
            .replace(cmd.as_std().get_program().to_str().unwrap(), &self.runner.to_string());
        let envs = cmd.envs_string(&self.runner.buildnoop_envs());

        let (status, effects) = match run_build(cmd, &call, containerfile, target, out_dir).await {
            Ok((status, effects)) => (status, effects),
            Err(e) => return rtrn(e),
        };

        // Something is very wrong here. Try to be helpful by logging some info about runner config:
        if !status.success() {
            let logs = env::var(ENV_LOG_PATH)
                .map(|val| format!("\nCheck logs at {val}"))
                .unwrap_or_default();
            return rtrn(anyhow!(
                "Runner failed.{logs}
Please report an issue along with information from the following:
* {runner} buildx version
* {runner} info
* {runner} buildx ls
* cargo green supergreen env
",
                runner = self.runner,
            ));
        }

        (call, envs, Ok(effects))
    }
}

// NOTE: build may fail with the below error (macOS):
//=> reason has something to do with `"credsStore": "desktop"` in ~/.docker/config.json
//
// [+] Building 0.4s (2/2) FINISHED
//  => [internal] load build definition from Dockerfile
//  => => transferring dockerfile: 382B
//  => ERROR resolve image config for docker-image://docker.io/docker/dockerfile:1@sha256:b6afd42430b15f2d2a4c5a02b919e98a525b785b1aaff16747d2f623364e39b6
// ------
//  > resolve image config for docker-image://docker.io/docker/dockerfile:1@sha256:b6afd42430b15f2d2a4c5a02b919e98a525b785b1aaff16747d2f623364e39b6:
// ------
// Dockerfile:1
// --------------------
//    1 | >>> # syntax=docker.io/docker/dockerfile:1@sha256:b6afd42430b15f2d2a4c5a02b919e98a525b785b1aaff16747d2f623364e39b6
//    2 |     # check=error=true
//    3 |     # Generated by https://github.com/fenollp/supergreen v0.19.0
// --------------------
// ERROR: failed to build: failed to solve: error getting credentials - err: exec: "docker-credential-desktop": executable file not found in $PATH, out: ``
// Error: # syntax=docker.io/docker/dockerfile:1@sha256:b6afd42430b15f2d2a4c5a02b919e98a525b785b1aaff16747d2f623364e39b6
// # check=error=true
// # Generated by https://github.com/fenollp/supergreen v0.19.0
// FROM --platform=$BUILDPLATFORM docker.io/library/rust:1.90.0-slim@sha256:e4ae8ab67883487c5545884d5aa5ebbe86b5f13c6df4a8e3e2f34c89cedb9f54 AS rust-base
// Unable to build rust-base: Runner failed.

async fn run_build(
    mut cmd: Command,
    call: &str,
    containerfile: &Utf8Path,
    target: &Stage,
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
        let out_path = stdio_path(target, out_dir, STDOUT);
        let err_path = stdio_path(target, out_dir, STDERR);
        let rcd_path = stdio_path(target, out_dir, ERRCODE);

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
                Ok(Ok(Ok(Accumulated { stdout, .. }))), //                      STDOUT
                Ok(Ok(Ok(Accumulated { stderr, written, envs, libs, .. }))), // STDERR
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
        Ok(workaround_missing_rmeta_or_rlib(acc))
    })
}

/// Sometimes, cargo's STDERR is missing some rlibs...
///
/// TODO: report to upstream cargo and investigate.
///
/// Mimic cargo's ordering: .d then .rmeta then .rlib
fn workaround_missing_rmeta_or_rlib(mut acc: Accumulated) -> Accumulated {
    if acc.written.iter().any(|f| f.extension() == Some("d")) {
        let rmeta = acc.written.iter().find(|f| f.extension() == Some("rmeta"));
        let rlib = acc.written.iter().find(|f| f.extension() == Some("rlib"));
        match (rmeta, rlib) {
            (Some(rmeta), None) if rmeta.with_extension("rlib").exists() => {
                acc.written.push(rmeta.with_extension("rlib"))
            }
            (None, Some(rlib)) if rlib.with_extension("rmeta").exists() => {
                acc.written.push(rlib.with_extension("rmeta"));
                let last = acc.written.len() - 1;
                acc.written.swap(1, last);
            }
            _ => {}
        }
    }
    acc
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

    if let Some(var) = rechrome::env_not_comptime_defined(&msg) {
        acc.envs.insert(var.to_owned());
        if let Some(new_msg) = rechrome::suggest_set_envs(var, &msg) {
            info!("suggesting to passthrough missing env with set-envs {var:?}");
            msg = new_msg;
        }
    }

    if let Some(lib) = rechrome::lib_not_found(&msg) {
        acc.libs.insert(lib.to_owned());
        if let Some(new_msg) = rechrome::suggest_add(lib, &msg) {
            info!("suggesting to add lib to base image {lib:?}");
            msg = new_msg;
        }
    }

    hide_credentials_on_rate_limit(&mut msg);

    eprintln!("{msg}");
    acc.stderr.push(msg);
}

/// Somehow, GitHub Actions won't hide this secret
fn hide_credentials_on_rate_limit(msg: &mut String) {
    const LHS: &str = "toomanyrequests: You have reached your pull rate limit as '";
    const SEP: &str = "': ";
    const RHS: &str =
        ". You may increase the limit by upgrading. https://www.docker.com/increase-rate-limit";
    if let Some(("", rest)) = msg.split_once(LHS) {
        if let Some((userpart, rest)) = rest.split_once(SEP) {
            if let Some((secret, "")) = rest.split_once(RHS) {
                *msg = format!("{LHS}{userpart}{SEP}{}{RHS}", "*".repeat(secret.len()));
            }
        }
    }
}

#[test]
fn hide_credentials_from_final_log() {
    let mut msg = "toomanyrequests: You have reached your pull rate limit as 'hubuser': dckr_jti_tookeeeennn-H0jyHY3m7bYZruA=. You may increase the limit by upgrading. https://www.docker.com/increase-rate-limit".to_owned();
    hide_credentials_on_rate_limit(&mut msg);
    assert_eq!(msg,
        "toomanyrequests: You have reached your pull rate limit as 'hubuser': *************************************. You may increase the limit by upgrading. https://www.docker.com/increase-rate-limit");
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
