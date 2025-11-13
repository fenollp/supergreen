use std::{
    env,
    error::Error,
    fs::OpenOptions,
    io::Write,
    ops::Not,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    process::{ExitStatus, Stdio},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Result};
use atomicwrites::{AtomicFile, OverwriteBehavior};
use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexSet;
use log::{debug, info};
use reqwest::Client as ReqwestClient;
use serde::Deserialize;
use tokio::{
    fs::File as TokioFile,
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader as TokioBufReader},
    join,
    process::Command,
    spawn,
    sync::oneshot::{self},
    task::JoinHandle,
};
use tokio_stream::StreamExt;

use crate::{
    du::lock_from_builder_cache,
    ext::{timeout, CommandExt},
    green::Green,
    image_uri::ImageUri,
    md::BuildContext,
    r#final::is_primary,
    rechrome,
    runner::DOCKER_HOST,
    stage::Stage,
    ENV_LOG_PATH, PKG,
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
        let req = req.map_err(|e| {
            // e.source(): try to be a bit more helpful than just "error sending request for url"
            anyhow!("Failed to build a request against {domain}: {e} ({:?})", e.source())
        })?;

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

        if self.repro() {
            cmd.arg("--no-cache");
        }

        for img in self.cache.from_images.iter().chain(self.cache.images.iter()) {
            let img = img.noscheme();
            cmd.arg(format!("--cache-from=type=registry,ref={img}"));
        }

        if !self.cache.to_images.is_empty() || !self.cache.images.is_empty() {
            let maxready = !self.builder.is_default();
            for img in self.cache.to_images.iter().chain(self.cache.images.iter()) {
                let img = img.noscheme();
                cmd.arg(format!(
                    "--cache-to=type=registry,ref={img}{mode}{compression},ignore-error={ignore_error}",

                    // ERROR: Cache export is not supported for the docker driver.
                    // Switch to a different driver, or turn on the containerd image store, and try again.
                    // Learn more at https://docs.docker.com/go/build-cache-backends/
                    mode = if maxready { ",mode=max" } else { "" },

                    // TODO? compression=zstd,force-compression=true
                    compression = "",

                    // TODO? if error when registry is unreachable, possible setting language: =1:my.org;0:some.org 1|0
                    ignore_error = "false",
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

        cmd.arg(format!("--network={}", self.base.with_network));

        cmd.arg("--platform=local");
        cmd.arg("--pull=false");
        cmd.arg(format!("--target={target}"));

        // // https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry#labelling-container-images
        // cmd.arg(format!("--label=org.opencontainers.image.description={target}"));

        if out_dir.is_some() {
            cmd.arg("--output=type=tar");
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
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());
        }

        let call = cmd.show();
        info!("Starting `{envs} {call} <{containerfile}`", envs = cmd.envs_string(&[]));
        eprintln!("Starting `{envs} {call} <{containerfile}`", envs = cmd.envs_string(&[]));
        let call = call
            .split_whitespace()
            .filter(|flag| !self.runner.buildnoop_flags().any(|prefix| flag.starts_with(prefix)))
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
            let logs = env::var(ENV_LOG_PATH!())
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

    let handles = if let Some(out_dir) = out_dir {
        let dbg_out: JoinHandle<Result<_>> = spawn({
            let mut out = TokioBufReader::new(child.stdout.take().expect("started"));
            let target = target.to_owned();
            let out_dir = out_dir.to_owned();
            let mut err_handle = None;
            let mut out_handle = None;
            let mut rcd = None;
            let mut written = vec![];
            async move {
                let mut buf = Vec::new();
                out.read_to_end(&mut buf)
                    .await
                    .map_err(|e| anyhow!("Failed getting all the buffer: {e}"))?;
                debug!("produced {target} 0x{}", sha256::digest(&buf));
                let out = TokioBufReader::new(buf.as_slice());
                let out_path = format!("{target}-{STDOUT}");
                let err_path = format!("{target}-{STDERR}");
                let rcd_path = format!("{target}-{ERRCODE}");

                info!("running untar on STDOUT");
                let mut ar = tokio_tar::Archive::new(out);
                let mut entries = ar.entries().map_err(|e| anyhow!("Failed reading TAR: {e}"))?;
                while let Some(Ok(mut f)) = entries.next().await {
                    let name: Utf8PathBuf = f
                        .path()
                        .map_err(|e| anyhow!("Failed decoding TAR entry name: {e}"))?
                        .to_string_lossy()
                        .to_string()
                        .into();

                    if name == out_path {
                        let mut buf = String::new();
                        f.read_to_string(&mut buf)
                            .await
                            .map_err(|e| anyhow!("Failed unTARing buffer: {e}"))?;
                        debug!("produced {name} 0x{}", sha256::digest(&buf));
                        out_handle = Some(buf);
                    } else if name == err_path {
                        let mut buf = String::new();
                        f.read_to_string(&mut buf)
                            .await
                            .map_err(|e| anyhow!("Failed unTARing buffer: {e}"))?;
                        debug!("produced {name} 0x{}", sha256::digest(&buf));
                        err_handle = Some(buf);
                    } else if name == rcd_path {
                        let line = TokioBufReader::new(f).lines().next_line().await;
                        rcd = line.ok().flatten().and_then(|x| x.parse::<i8>().ok());
                    } else {
                        written.push(name.clone());
                        info!("creating (RW) {name:?}");
                        let fname = out_dir.join(&name);
                        let mode =
                            f.header().mode().map_err(|e| anyhow!("Failed decoding mode: {e}"))?;

                        // Let's drop async for FS operations: we're not writing gigabytes!
                        // Also: entries MUST be consumed in sequence anyway.
                        let mut buf = Vec::new();
                        f.read_to_end(&mut buf)
                            .await
                            .map_err(|e| anyhow!("Failed unTARing buffer: {e}"))?;
                        debug!("produced {name} 0x{}", sha256::digest(&buf));

                        let atomix = AtomicFile::new(&fname, OverwriteBehavior::AllowOverwrite);
                        let mut options = OpenOptions::new();
                        options.read(true).write(true).create(true).truncate(true).mode(mode);
                        atomix
                            .write_with_options(|f| f.write_all(&buf), options)
                            .map_err(|e| anyhow!("Failed writing unTARed: {e}"))?;

                        assert_eq!(f.link_name().unwrap(), None);
                        assert_eq!(f.header().entry_type().as_byte(), 0x30);
                        assert_eq!(f.header().uid().unwrap(), 0);
                        assert_eq!(f.header().gid().unwrap(), 0);
                        //assert_eq!(f.header().mtime().unwrap(), 42);
                        assert_eq!(f.header().username(), Ok(Some("")));
                        assert_eq!(f.header().groupname(), Ok(Some("")));

                        assert_eq!(
                            fname.metadata().unwrap().mode() & 0o777,
                            mode,
                            "Unexpected untared-then-written file mode {:#o} vs: {mode:#o} {:?}",
                            fname.metadata().unwrap().mode(),
                            fname.metadata()
                        );
                    }
                }
                info!("rustc wrote {} files:", written.len());
                written.sort();
                Ok((out_handle, err_handle, rcd, written))
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

    let mut effects = Effects { written: vec![], stdout: vec![], stderr: vec![] };
    if let Some((dbg_out, dbg_err)) = handles {
        if let Ok(e) = rx_err.try_recv() {
            bail!("Runner BUG: {e}")
        }
        // NOTE:
        // * if call to rustc fails, errcode file will exist but the build will complete.
        // * if the call doesn't fail, the file isn't created.
        // * if the build fails that's a bug, and no files will be outputed.
        match join!(timeout(dbg_out), timeout(dbg_err)) {
            (Ok(Ok(Ok((_, _, Some(errcode), _)))), _) => {
                bail!("Runner failed with exit code {errcode}")
            }
            (Ok(Ok(Err(e))), _) => {
                bail!("Something went wrong (maybe retry?): {e}")
            }
            (Ok(Ok(Ok((Some(out_buf), Some(err_buf), _, written)))), _) => {
                let Accumulated { stdout, .. } = fwd(&out_buf, "➤", fwd_stdout);
                let Accumulated { stderr, envs, libs, .. } = fwd(&err_buf, "✖", fwd_stderr);
                info!("Suggested {PKG}-specific config: envs:{} libs:{}", envs.len(), libs.len());
                effects.stdout = stdout;
                effects.stderr = stderr;
                effects.written = written;
            }
            (e1, e2) => {
                bail!("BUG: STDIO forwarding crashed: {e1:?} | {e2:?}")
            }
        }
    }
    drop(child);
    Ok((status, effects))
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
fn fwd(
    stdio: &str,
    badge: &'static str,
    fwd_std: impl Fn(&str, &mut Accumulated) + Send + 'static,
) -> Accumulated {
    let mut acc = Accumulated::default();
    for line in stdio.lines() {
        if line.is_empty() {
            continue;
        }

        debug!("{badge} {}", strip_ansi_escapes(line));

        if let Some(msg) = lift_stdio(line) {
            fwd_std(msg, &mut acc);
        }

        // //warning: panic message contains an unused formatting placeholder
        // //--> /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/proc-macro2-1.0.36/build.rs:191:17
        // FIXME un-rewrite /index.crates.io-0000000000000000/ in cargo messages
        // => also in .d files
        // cache should be ok (cargo's point of view) if written right after green's build(..) call
    }
    acc
}

#[derive(Debug, Default)]
struct Accumulated {
    envs: IndexSet<String>,
    libs: IndexSet<String>,
    stdout: Vec<String>,
    stderr: Vec<String>,
}

fn fwd_stderr(msg: &str, acc: &mut Accumulated) {
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
fn lift_stdio(line: &str) -> Option<&str> {
    // Docker builds running shell code usually start like: #47 0.057
    let line = line.trim_start_matches(|c| ['#', '.', ' '].contains(&c) || c.is_ascii_digit());
    let msg = line.trim();
    msg.is_empty().not().then_some(msg)
}
