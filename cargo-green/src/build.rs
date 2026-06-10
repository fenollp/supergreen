use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    env,
    fs::DirBuilder,
    io::Write,
    ops::Not,
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        process::ExitStatusExt,
    },
    process::{ExitStatus, Stdio},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Error, Result};
use atomic_write_file::AtomicWriteFile;
use camino::{Utf8Path, Utf8PathBuf};
use indexmap::{IndexMap, IndexSet};
use log::{debug, info, warn};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    join,
    process::{ChildStderr, ChildStdin, ChildStdout, Command},
    spawn,
    sync::oneshot::{self, Sender},
    time::{error::Elapsed, timeout},
};
use tokio_stream::StreamExt;
use tokio_tar::{Archive as TarArchive, Entry as TarEntry, EntryType, Header as TarHeader};

use crate::{
    base_image::un_rewrite_cargo_home,
    cache::result::{assert_tarball_header, ResultWriter},
    dirs::Dirs,
    ext::CommandExt,
    green::Green,
    md::{BuildContext, DIESES},
    r#final::is_primary,
    rechrome,
    runner::Runner,
    stage::Stage,
    target_dir::un_virtual_target_dir_str,
    PKG,
};

pub(crate) const ERRCODE: &str = "errcode";
pub(crate) const STDERR: &str = "stderr";
pub(crate) const STDOUT: &str = "stdout";

/// Value of BuildKit build-arg cf <https://reproducible-builds.org/docs/source-date-epoch>
// TODO? use a non-fixed EPOCH value
// * set SOURCE_DATE_EPOCH=$(git log -1 --pretty=%ct) for local code, and
// * set it to crates' birth date, in case it's a $HOME/.cargo/registry/cache/...crate
// * set it to the directory's birth date otherwise (should be a relative path to local files).
//
// https://github.com/moby/buildkit/releases/tag/dockerfile%2F1.24.0-rc1
// Dockerfile now supports special arg definitions SOURCE_DATE_EPOCH=context and SOURCE_DATE_EPOCH=<stage>
// which set the value of SOURCE_DATE_EPOCH to the timestamp associated with the remote context or the stage respectively.
// When building from a Git commit, the context timestamp is the commit timestamp, and when building from a remote URL,
// the timestamp is resolved from the metadata of files in the TAR archive or from the Last-Modified header of the URL #6602
pub(crate) const SOURCE_DATE_EPOCH: u64 = 42;

impl Green {
    pub(crate) async fn build_cacheonly(
        &self,
        containerfile: &Utf8Path,
        target: &Stage,
    ) -> Result<()> {
        self.build(containerfile, target, &[].into(), None, None).await.4
    }

    pub(crate) async fn build_out(
        &self,
        containerfile: &Utf8Path,
        target: &Stage,
        contexts: &IndexSet<BuildContext>,
        out_dir: &Utf8Path,
    ) -> (String, String, Effects, Option<ResultWriter>, Result<()>) {
        let (built, cached) = join!(biased;
            self.build(containerfile, target, contexts, Some(out_dir), None),
            async {
                let true = self.runner.is_buildkit() else { return Ok(()) };
                // Concurrently run same build just to export runner cache
                let true = self.cachebuildkit() else { return Ok(()) }; // TODO: drop experiment
                let Some(ref dirs) = self.dirs else { return Ok(()) };
                let Some(dst) = dirs.new_runner_cache(target)? else { return Ok(()) };
                self.build(containerfile, target, contexts, None, Some(&dst)).await.4
            }
        );
        if let Err(e) = cached {
            if built.4.is_ok() {
                warn!("troubles saving runner cache: {e}");
            }
        }
        built
    }

    async fn build(
        &self,
        containerfile: &Utf8Path,
        target: &Stage,
        contexts: &IndexSet<BuildContext>,
        out_dir: Option<&Utf8Path>,
        export: Option<&Utf8Path>,
    ) -> (String, String, Effects, Option<ResultWriter>, Result<()>) {
        assert!(!self.runner.is_none(), "build() called with Runner::None");
        let mut cmd = match self.cmd() {
            Ok(cmd) => cmd,
            Err(e) => return ("".to_owned(), "".to_owned(), Effects::default(), None, Err(e)),
        };
        cmd.arg("build");

        let (call, envs) =
            self.with_docker_args(&mut cmd, containerfile, target, contexts, out_dir, export);

        let mut effects = Effects::default();
        let (status, result) =
            match self.run_build(&mut effects, cmd, &call, containerfile, target, out_dir).await {
                Ok((status, result)) => (status, result),
                Err(e) => return (call, envs, effects, None, Err(e)),
            };

        // Something is very wrong here. Try to be helpful by logging some info about runner config:
        if !status.success() {
            let e = effects.try_to_help(&self.runner, self.cargo_home.as_str());
            return (call, envs, effects, result, Err(e));
        }

        (call, envs, effects, result, Ok(()))
    }

    fn with_docker_args(
        &self,
        cmd: &mut Command,
        containerfile: &Utf8Path,
        target: &Stage,
        contexts: &IndexSet<BuildContext>,
        out_dir: Option<&Utf8Path>,
        export: Option<&Utf8Path>,
    ) -> (String, String) {
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

        if let Some(dst) = export {
            cmd.arg(self.builder.export_arg(dst));
        }
        if let Some(ref dirs) = self.dirs {
            if self.runner.is_buildkit() && self.cachebuildkit() {
                if let Some(src) = dirs.runner_cache(target) {
                    cmd.arg(self.builder.import_arg(&src));
                }
            }
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
        } else {
            // cacheonly: tee to CLI + to Effects.stderr to try_to_help
            cmd.stderr(Stdio::piped());
        }

        let call = cmd.show();
        let envs = cmd.envs_string(&self.runner.buildnoop_envs());
        info!("Starting `{envs} {call} <{containerfile}`");
        eprintln!("Starting `{envs} {call} <{containerfile}`");
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

        (call, envs)
    }

    async fn run_build(
        &self,
        effects: &mut Effects,
        mut cmd: Command,
        call: &str,
        containerfile: &Utf8Path,
        target: &Stage,
        out_dir: Option<&Utf8Path>,
    ) -> Result<(ExitStatus, Option<ResultWriter>)> {
        let start = Instant::now();
        let mut child = cmd.spawn().map_err(|e| anyhow!("Failed starting `{call}`: {e}"))?;

        spawn({
            let containerfile = containerfile.to_owned();
            let stdin = child.stdin.take().expect("started");
            async move { send_containerfile(stdin, containerfile).await }
        });

        // ---

        let pid = child.id().unwrap_or_default();
        info!("Started as pid={pid} in {:?}", start.elapsed());

        let (tx_err, mut rx_err) = oneshot::channel();

        let (handles, tee_err) = if let Some(out_dir) = out_dir {
            let dbg_out = spawn({
                let target = target.to_owned();
                let out_dir = out_dir.to_owned();
                let dirs = self.dirs.clone();
                let cargo_home = self.cargo_home.to_string();
                let stdout = child.stdout.take().expect("started");
                async move { build_stdout(stdout, target, out_dir, dirs, cargo_home).await }
            });

            let dbg_err = spawn({
                let stderr = child.stderr.take().expect("started");
                async move { build_stderr(stderr, Some(tx_err)).await }
            });

            (Some((dbg_out, dbg_err)), None)
        } else {
            let tee_err = spawn({
                let stderr = child.stderr.take().expect("started");
                async move { tee_stderr(stderr).await }
            });

            (None, Some(tee_err))
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
        info!("build ran for {secs:?}");

        if let Ok(e) = rx_err.try_recv() {
            bail!("Runner BUG: {e}")
        }

        // NOTE:
        // * if call to rustc fails, errcode file will exist but the build will complete.
        // * if the call doesn't fail, the file isn't created.
        // * if the build fails that's a bug, and no files will be outputed.

        const SOME_TIME: Duration = Duration::from_mins(30);

        let Some((dbg_out, dbg_err)) = handles else {
            let Some(tee_err) = tee_err else { unreachable!("either handles or tee_err") };
            let joined = timeout(SOME_TIME, tee_err).await;
            drop(child);
            match joined {
                Ok(Ok(err_buf)) => {
                    // Keep STDERR around so Effects::try_to_help can match on errors even for cacheonly
                    effects.stderr = err_buf.lines().map(ToOwned::to_owned).collect();
                }
                Ok(Err(e)) if e.is_cancelled() => bail!("BUG: stderr tee was cancelled: {e}"),
                Ok(Err(e)) => bail!("BUG: stderr tee panic'd or crashed: {e}"),
                Err(Elapsed { .. }) => bail!("BUG: build took longer than {SOME_TIME:?}"),
            }
            return Ok((status, None));
        };
        let joined = join!(timeout(SOME_TIME, dbg_out), timeout(SOME_TIME, dbg_err));
        drop(child);

        match joined {
            (Err(Elapsed { .. }), _) | (_, Err(Elapsed { .. })) => {
                bail!("BUG: build took longer than {SOME_TIME:?}")
            }
            (Ok(Err(e)), _) | (_, Ok(Err(e))) if e.is_cancelled() => {
                bail!("BUG: build was cancelled: {e}")
            }
            (Ok(Err(e)), _) | (_, Ok(Err(e))) => {
                bail!("BUG: build panic'd or crashed: {e}")
            }
            (Ok(Ok(Err(e))), _) => {
                bail!("Something went wrong (maybe retry?): {e}")
            }
            (Ok(Ok(Ok((out_buf, err_buf, errcode, written, result)))), _) => {
                let FromStdout { stdout, rustc_envs } = fwd_stdout(&out_buf, "➤", &self.cargo_home);
                info!("Buildscript {PKG}-specific config: envs:{}", rustc_envs.len());
                effects.cargo_rustc_env = rustc_envs;

                let FromStderr { stderr, envs, libs } = fwd_stderr(&err_buf, "✖", &self.cargo_home);
                info!("Suggested {PKG}-specific config: envs:{} libs:{}", envs.len(), libs.len());
                effects.stdout = stdout;
                effects.stderr = stderr;
                effects.written = written;

                Ok((errcode.map(ExitStatus::from_raw).unwrap_or(status), result))
            }
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Effects {
    pub(crate) written: Vec<Utf8PathBuf>,
    pub(crate) stdout: Vec<String>,
    pub(crate) stderr: Vec<String>,
    pub(crate) cargo_rustc_env: IndexMap<String, String>,
}

impl Effects {
    fn try_to_help(&self, runner: &Runner, cargo_home: &str) -> Error {
        let rewrite = |msg: &str| {
            let msg = un_virtual_target_dir_str(msg);
            un_rewrite_cargo_home(&msg, cargo_home)
        };

        let cargo_msgs = |legacy, pat, it: core::slice::Iter<'_, String>| -> String {
            it.filter_map(|line| {
                line.split_once(legacy).xor(line.split_once(pat)).map(|(_, rhs)| rhs)
            })
            .map(rewrite)
            .collect::<Vec<_>>()
            .join("\n")
        };

        // https://doc.rust-lang.org/cargo/reference/build-scripts.html#cargo-warning
        let cargo_warnings = cargo_msgs("cargo:warning=", "cargo::warning=", self.stdout.iter());
        // https://doc.rust-lang.org/cargo/reference/build-scripts.html#cargo-error
        let cargo_errors = cargo_msgs("cargo:error=", "cargo::error=", self.stdout.iter());
        if !cargo_warnings.is_empty() || !cargo_errors.is_empty() {
            return anyhow!("Runner failed.\n{cargo_warnings}\n{cargo_errors}\n");
        }

        if self.stderr.iter().any(|line| line.contains("you have held broken packages")) {
            let pkgs = broken_packages(self.stderr.iter().map(AsRef::as_ref));
            if !pkgs.is_empty() {
                return anyhow!("Unable to install these system packages: {pkgs:?}");
            }
        }

        if failed_downloading(self.stderr.iter().map(AsRef::as_ref)) {
            return anyhow!("Failed while downloading a crate's source code, please check your connection and try again");
        }
        if buildkit_interrupted(self.stderr.iter().map(AsRef::as_ref)) {
            return anyhow!("Runner daemon was possibly restarted, please try again");
        }

        let logs = env::var(ENV_LOG_PATH!())
            .map(|val| format!("\nCheck logs at {val}"))
            .unwrap_or_default();
        anyhow!(
            "Runner failed.{logs}\n{stdout}\n{stderr}\n
Please report an issue along with information from the following:
* {runner} buildx version
* {runner} info
* {runner} buildx ls
* cargo green supergreen env
",
            stdout = self.stdout.iter().map(|x| rewrite(x)).collect::<Vec<_>>().join("\n"),
            stderr = self.stderr.iter().map(|x| rewrite(x)).collect::<Vec<_>>().join("\n"),
        )
    }
}

async fn send_containerfile(mut stdin: ChildStdin, containerfile: Utf8PathBuf) -> Result<()> {
    let reader = File::open(&containerfile)
        .await
        .map_err(|e| anyhow!("Failed opening (RO) {containerfile}: {e}"))?;

    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.starts_with(DIESES) {
            continue;
        }

        let errf = |e| anyhow!("Failed piping containerfile: {e}");
        stdin.write_all(line.as_bytes()).await.map_err(errf)?;
        stdin.write_u8(b'\n').await.map_err(errf)?;
    }
    Ok(())
}

async fn build_stdout(
    stdout: ChildStdout,
    target: Stage,
    out_dir: Utf8PathBuf,
    dirs: Option<Dirs>,
    cargo_home: String,
) -> Result<(String, String, Option<i32>, Vec<Utf8PathBuf>, Option<ResultWriter>)> {
    let mut result = if let Some(ref dirs) = dirs { dirs.new_result(&target).await? } else { None };

    info!("running untar on STDOUT");
    let mut buf = Vec::new();
    BufReader::new(stdout)
        .read_to_end(&mut buf)
        .await
        .map_err(|e| anyhow!("Failed getting all the buffer: {e}"))?;
    debug!("produced {target} {}B 0x{}", buf.len(), sha256::digest(&buf));
    if let Some(ref mut result) = result {
        result.add_tarball(&buf).await?;
    }

    let (out_handle, err_handle, rcd, written) =
        untar_into(&buf, &target, &out_dir, &cargo_home).await?;
    Ok((out_handle, err_handle, rcd, written, result))
}

async fn untar_into(
    buf: &[u8],
    target: &Stage,
    out_dir: &Utf8Path,
    cargo_home: &str,
) -> Result<(String, String, Option<i32>, Vec<Utf8PathBuf>)> {
    let mut err_handle = String::new();
    let mut out_handle = String::new();
    let mut rcd = None;
    let mut written = vec![];

    let mut ar = TarArchive::new(BufReader::new(buf));
    let mut entries = ar.entries().map_err(|e| anyhow!("Failed reading TAR: {e}"))?;
    while let Some(Ok(mut f)) = entries.next().await {
        let name: Utf8PathBuf = f
            .path()
            .map_err(|e| anyhow!("Failed decoding TAR entry name: {e}"))?
            .to_string_lossy()
            .to_string()
            .into();

        // No async: entries MUST be consumed in sequence
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).await.map_err(|e| anyhow!("Failed unTARing {name}: {e}"))?;
        let header = f.header();
        debug!("produced {}B {name} 0x{}", buf.len(), sha256::digest(&buf));

        match name.as_str().trim_start_matches(&format!("{target}-")) {
            STDOUT => {
                out_handle =
                    String::from_utf8(buf).map_err(|e| anyhow!("Corrupted result STDOUT: {e}"))?
            }
            STDERR => {
                err_handle =
                    String::from_utf8(buf).map_err(|e| anyhow!("Corrupted result STDERR: {e}"))?
            }
            ERRCODE => {
                let line = BufReader::new(f).lines().next_line().await;
                rcd = line.ok().flatten().and_then(|x| x.parse::<i32>().ok());
            }
            _ => {
                written.push(name.clone());
                info!("creating (RW) {name:?}");
                let fname = out_dir.join(&name);
                write_build_artifact(header, cargo_home, fname, buf, &f)?;
            }
        }
    }
    info!("rustc wrote {} files:", written.len());
    written.sort();
    Ok((out_handle, err_handle, rcd, written))
}

fn write_build_artifact(
    header: &TarHeader,
    cargo_home: &str,
    fname: Utf8PathBuf,
    buf: Vec<u8>,
    f: &TarEntry<TarArchive<BufReader<&[u8]>>>,
) -> Result<()> {
    let mode = header.mode().map_err(|e| anyhow!("Corrupted result mode: {e}"))?;

    assert_tarball_header(header);

    match header.entry_type() {
        EntryType::Regular => {
            info!("opening (Watomic) file {fname}");
            let mut opts = AtomicWriteFile::options();
            opts.mode(mode);
            let mut file =
                opts.open(&fname).map_err(|e| anyhow!("Failed opening atomic {fname}: {e}"))?;
            if fname.as_str().ends_with(".d") {
                let buf = str::from_utf8(&buf).map_err(|e| anyhow!("Corrupted result .d: {e}"))?;
                // NOTE: rewrite text here so cargo shows host paths and keeps the illusion
                // but really binaries (rlib, rmeta and such) cannot be modified.
                let buf = un_virtual_target_dir_str(buf);
                let buf = un_rewrite_cargo_home(&buf, cargo_home);
                file.write_all(buf.as_bytes())
            } else {
                file.write_all(&buf)
            }
            .map_err(|e| anyhow!("Failed writing unTARed: {e}"))?;
            file.commit().map_err(|e| anyhow!("Failed committing unTARed: {e}"))?;
        }

        EntryType::Directory => {
            info!("creating path {fname}");
            DirBuilder::new()
                .mode(mode)
                .recursive(true) //= mkdir "-p"
                .create(&fname)
                .map_err(|e| anyhow!("Failed `mkdir -p {fname}`: {e}"))?;
        }

        EntryType::Symlink => {
            info!("creating symlink {fname}");
            let name =
                f.link_name().map_err(|e| anyhow!("Failed reading link name of {fname}: {e}"))?;
            let Some(name) = name else { bail!("Link name not present for {fname}") };
            let _ = symlink::remove_symlink_file(&fname);
            symlink::symlink_file(&name, &fname)
                .map_err(|e| anyhow!("Failed `ln -s {name:?} {fname}`: {e}"))?;
        }

        entryty => bail!("BUG: unexpected entry type {entryty:?}"),
    }

    assert_eq!(
        fname.symlink_metadata().unwrap_or_else(|e| panic!("{fname}: {e}")).mode() & 0o777,
        mode,
        "Unexpected untared-then-written file mode {:#o} vs: {mode:#o} {:?} for {fname}",
        fname.symlink_metadata().unwrap_or_else(|e| panic!("{fname}: {e}")).mode(),
        fname.symlink_metadata()
    );
    Ok(())
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

async fn build_stderr(stderr: ChildStderr, mut tx_err: Option<Sender<String>>) -> Result<()> {
    let mut lines = BufReader::new(stderr).lines();

    let mut details: BTreeMap<String, String> = [].into();
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
            let Some((ctx, value)) = detail.split_once(':') else { continue };
            details
                .entry(ctx.to_owned())
                .and_modify(|v| *v = value.to_owned())
                .or_insert(value.to_owned());
        }

        // Count DONEs and CACHEDs
        if line.contains(" DONE ") {
            dones += 1;
        } else if line.ends_with(" CACHED") {
            cacheds += 1;
        }
    }
    info!("Terminating task CACHED:{cacheds} DONE:{dones} {details:?}");
    Ok(())
}

/// Show in cli but still keep ~1MB rolling text buffer stderr
async fn tee_stderr(stderr: ChildStderr) -> String {
    let mut lines = BufReader::new(stderr).lines();
    let mut ring = VecDeque::new();
    while let Ok(Some(line)) = lines.next_line().await {
        eprintln!("{line}"); // Tee to CLI

        ring.extend(line.as_bytes());
        ring.push_back(b'\n');
        const CAP: usize = 1 << 20; // 1 MiB
        while ring.len() > CAP {
            ring.pop_front();
        }
    }
    String::from_utf8_lossy(ring.make_contiguous()).into_owned()
}

#[derive(Debug, Default)]
struct FromStderr {
    stderr: Vec<String>,
    envs: IndexSet<String>,
    libs: IndexSet<String>,
}

fn fwd_stderr(stderr: &str, badge: &'static str, cargo_home: &Utf8Path) -> FromStderr {
    let mut acc = FromStderr::default();
    for line in stderr.lines() {
        if line.is_empty() {
            continue;
        }

        debug!("{badge} {}", strip_ansi_escapes(line));

        if let Some(msg) = lift_stdio(line) {
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

            acc.stderr.push(msg.clone());
            let msg = un_virtual_target_dir_str(&msg);
            let msg = un_rewrite_cargo_home(&msg, cargo_home.as_str());
            eprintln!("{msg}");
        }
    }
    acc
}

#[derive(Debug, Default)]
struct FromStdout {
    stdout: Vec<String>,
    rustc_envs: IndexMap<String, String>,
}

fn fwd_stdout(stdout: &str, badge: &'static str, cargo_home: &Utf8Path) -> FromStdout {
    let mut acc = FromStdout::default();
    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }

        debug!("{badge} {}", strip_ansi_escapes(line));

        if let Some(msg) = lift_stdio(line) {
            info!("(To cargo's STDOUT): {msg}");

            if let Some((_, rhs)) = msg.split_once("cargo::").xor(msg.split_once("cargo:")) {
                // https://doc.rust-lang.org/cargo/reference/build-scripts.html#outputs-of-the-build-script
                // > MSRV: 1.77 is required for cargo::KEY=VALUE syntax. To support older versions, use the cargo:KEY=VALUE syntax.
                if rhs.starts_with("rerun-if-changed=") {
                    // PATH — Tells Cargo when to re-run the script.
                } else if rhs.starts_with("rerun-if-env-changed=") {
                    // VAR — Tells Cargo when to re-run the script.
                } else if rhs.starts_with("rustc-link-arg=") {
                    // FLAG — Passes custom flags to a linker for benchmarks, binaries, cdylib crates, examples, and tests.
                } else if rhs.starts_with("rustc-link-arg-cdylib=") {
                    // FLAG — Passes custom flags to a linker for cdylib crates.
                } else if rhs.starts_with("rustc-link-arg-bin=BIN=") {
                    // FLAG — Passes custom flags to a linker for the binary BIN.
                } else if rhs.starts_with("rustc-link-arg-bins=") {
                    // FLAG — Passes custom flags to a linker for binaries.
                } else if rhs.starts_with("rustc-link-arg-tests=") {
                    // FLAG — Passes custom flags to a linker for tests.
                } else if rhs.starts_with("rustc-link-arg-examples=") {
                    // FLAG — Passes custom flags to a linker for examples.
                } else if rhs.starts_with("rustc-link-arg-benches=") {
                    // FLAG — Passes custom flags to a linker for benchmarks.
                } else if rhs.starts_with("rustc-link-lib=") {
                    // LIB — Adds a library to link.
                } else if rhs.starts_with("rustc-link-search=") {
                    // [KIND=]PATH — Adds to the library search path.
                } else if rhs.starts_with("rustc-flags=") {
                    // FLAGS — Passes certain flags to the compiler.
                } else if rhs.starts_with("rustc-cfg=") {
                    // KEY[="VALUE"] — Enables compile-time cfg settings.
                } else if rhs.starts_with("rustc-check-cfg=") {
                    // CHECK_CFG – Register custom cfgs as expected for compile-time checking of configs.
                } else if rhs.starts_with("rustc-env=") {
                    // VAR=VALUE — Sets an environment variable.
                    if let Some((var, val)) = rhs.trim_start_matches("rustc-env=").split_once("=") {
                        // NOTE: cargo errors if second '=' doesn't exist
                        acc.rustc_envs.insert(var.to_owned(), val.to_owned());
                    }
                } else if rhs.starts_with("error=") {
                    // MESSAGE — Displays an error on the terminal.
                } else if rhs.starts_with("warning=") {
                    // MESSAGE — Displays a warning on the terminal.
                } else if rhs.starts_with("metadata=") {
                    // KEY=VALUE — Metadata, used by links scripts.
                } else {
                    // Probably the ≤1.77 way of passing metadata:
                    //   https://doc.rust-lang.org/cargo/reference/build-scripts.html#the-links-manifest-key
                    warn!("unexpected cargo directive {rhs:?}")
                    // e.g: crate zstd-sys prints cargo:include=/some/path
                    //   which cargo actually interprets as cargo::metadata=include=/some/path
                    //     which then sets env DEP_ZSTD_INCLUDE=/some/path
                }
            }

            acc.stdout.push(msg.to_owned());
            let msg = un_virtual_target_dir_str(msg);
            let msg = un_rewrite_cargo_home(&msg, cargo_home.as_str());
            println!("{msg}");
        }
    }
    acc
}

/// Extract system packages that apt-satisfy wasn't able to install
fn broken_packages<'a>(it: impl Iterator<Item = &'a str>) -> HashSet<&'a str> {
    it.filter_map(|line| line.split_once(" Depends: "))
        .map(|(_, rhs)| rhs)
        .filter_map(|line| line.split_once(" but it is not installable"))
        .map(|(lhs, _)| lhs)
        .collect()
}

#[test]
fn list_broken_packages() {
    let stderr = r#"
        #379 3.697 Building dependency tree...
        #379 3.807 Reading state information...
        #379 3.826 11 packages can be upgraded. Run 'apt list --upgradable' to see them.
        #379 3.827 + DEBIAN_FRONTEND=noninteractive xx-apt satisfy --no-install-recommends -y ca-certificates gcc libc6-dev libsqlite3-dev libssl-dev=3.5.5-1~deb13u2 pkg-config zlib1g-dev
        #379 3.837
        #379 3.837 WARNING: apt does not have a stable CLI interface. Use with caution in scripts.
        #379 3.837
        #379 3.840 Reading package lists...
        #379 4.169 Building dependency tree...
        #379 4.271 Reading state information...
        #379 4.323 Some packages could not be installed. This may mean that you have
        #379 4.323 requested an impossible situation or if you are using the unstable
        #379 4.323 distribution that some required packages have not yet been created
        #379 4.323 or been moved out of Incoming.
        #379 4.323 The following information may help to resolve the situation:
        #379 4.323
        #379 4.323 Unsatisfied dependencies:
        #379 4.388  satisfy:command-line : Depends: libssl-dev=3.5.5-1~deb13u2 but it is not installable
        #379 4.390 Error: Unable to correct problems, you have held broken packages.
"#;
    let pkgs = broken_packages(stderr.lines());
    assert_eq!(pkgs, ["libssl-dev=3.5.5-1~deb13u2"].into());
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

#[must_use]
fn failed_downloading<'a>(mut it: impl Iterator<Item = &'a str>) -> bool {
    #[expect(clippy::nonminimal_bool)]
    it.any(|line| {
        false
            || line.contains(" http2: server sent GOAWAY and closed the connection;")
            || line.contains(" http2: client connection force closed via ")
            || line.contains(" read: connection reset by peer")
            || line.contains(" failed to fetch remote ")
            || line.contains(" digest mismatch sha256:")
    })
}

#[test]
fn download_failed() {
    let stderrs = vec![
        r#"
        I 26/04/28 16:27:29.393 N str-buf 3.0.3 c0a72a922652c7f1 ✖ #14 ERROR: digest mismatch sha256:08bed0bc69739d1f4e553a9cb1a4db848332274df4257efc036db17ad02b9f15: sha256:0ceb97b7225c713c2fd4db0153cb6b3cab244eb37900c3f634ed4d43310d8c34
        I 26/04/28 16:27:29.436 N str-buf 3.0.3 c0a72a922652c7f1 ✖ ------
        I 26/04/28 16:27:29.436 N str-buf 3.0.3 c0a72a922652c7f1 ✖  > [cratesio-str-buf-3.0.3 1/1] ADD --chmod=0664 --unpack --checksum=sha256:0ceb97b7225c713c2fd4db0153cb6b3cab244eb37900c3f634ed4d43310d8c34   https://static.crates.io/crates/str-buf/str-buf-3.0.3.crate /:
        I 26/04/28 16:27:29.436 N str-buf 3.0.3 c0a72a922652c7f1 ✖ ------
        I 26/04/28 16:27:29.437 N str-buf 3.0.3 c0a72a922652c7f1 ✖ ERROR: failed to build: failed to solve: digest mismatch sha256:08bed0bc69739d1f4e553a9cb1a4db848332274df4257efc036db17ad02b9f15: sha256:0ceb97b7225c713c2fd4db0153cb6b3cab244eb37900c3f634ed4d43310d8c34
        "#,
        r#"
        E 26/04/28 16:27:29.442 N str-buf 3.0.3 c0a72a922652c7f1 Error: Runner BUG: failed to build: failed to solve: digest mismatch sha256:08bed0bc69739d1f4e553a9cb1a4db848332274df4257efc036db17ad02b9f15: sha256:0ceb97b7225c713c2fd4db0153cb6b3cab244eb37900c3f634ed4d43310d8c34
        "#,
        r#"
        E 26/05/22 13:59:46.807 N zerovec 0.11.4 77b613567de82307 Error: Runner BUG: failed to build: failed to solve: Get "https://static.crates.io/crates/zerovec/zerovec-0.11.4.crate": http2: server sent GOAWAY and closed the connection; LastStreamID=289, ErrCode=NO_ERROR, debug="graceful shutdown"
        "#,
        r#"
        E 26/06/02 00:03:45.481 X semver 1.0.26 9fbca58694034ec8 Error: Runner BUG: failed to build: failed to solve: Get "https://static.crates.io/crates/semver/semver-1.0.26.crate": http2: server sent GOAWAY and closed the connection; LastStreamID=257, ErrCode=NO_ERROR, debug="graceful shutdown"
        "#,
        r#"
        Error: Runner BUG: failed to build: failed to solve: failed to fetch remote https://codeberg.org/willempx/qair.git: git stderr:
        "#,
        r#"
        ERROR: failed to build: failed to solve: Get "https://static.crates.io/crates/windows_x86_64_gnullvm/windows_x86_64_gnullvm-0.52.6.crate": http2: client connection force closed via ClientConn.Close
        "#,
        r#"
        ERROR: failed to build: failed to solve: Get "https://static.crates.io/crates/yoke-derive/yoke-derive-0.8.1.crate": read tcp 172.17.0.2:43654->151.101.162.137:443: read: connection reset by peer
        "#,
    ];
    for stderr in stderrs {
        assert!(failed_downloading(stderr.lines()), "In: {stderr}");
    }
}

#[test]
fn un_rewrites_target_dir_before_outputting_to_cargo() {
    temp_env::with_var("CARGO_TARGET_DIR", Some("/some/path/"), || {
        let msg = r#"
    {"$message_type":"artifact","artifact":"/target/release/deps/libclap_derive-fcea659dae5440c4.so","emit":"link"}
    {"$message_type":"diagnostic","message":"2 warnings emitted","code":null,"level":"warning","spans":[],"children":[],"rendered":"warning: 2 warnings emitted\n\n"}
    hi!
    "#;
        assert_eq!(
            un_virtual_target_dir_str(msg),
            r#"
    {"$message_type":"artifact","artifact":"/some/path/release/deps/libclap_derive-fcea659dae5440c4.so","emit":"link"}
    {"$message_type":"diagnostic","message":"2 warnings emitted","code":null,"level":"warning","spans":[],"children":[],"rendered":"warning: 2 warnings emitted\n\n"}
    hi!
    "#
        );
    })
}

#[must_use]
fn buildkit_interrupted<'a>(mut it: impl Iterator<Item = &'a str>) -> bool {
    it.any(|line| line.contains(" received prior goaway:"))
}

#[test]
fn interrupted_runner() {
    let stderr = r#"
    > resolve image config for docker-image://docker.io/docker/dockerfile:1@sha256:2780b5c3bab67f1f76c781860de469442999ed1a0d7992a5efdf2cffc0e3d769:
    ------
    ERROR: failed to build: failed to receive status: rpc error: code = Unavailable desc = closing transport due to: connection error: desc = "error reading from server: EOF", received prior goaway: code: NO_ERROR, debug data: "graceful_stop"
    "#;
    assert!(buildkit_interrupted(stderr.lines()), "In: {stderr}");
}

#[test]
fn stdio_passthrough_from_runner() {
    assert_eq!(lift_stdio("#47 1.714 hi!"), Some("hi!"));
    let lines = [
        r#"#47 1.714 {"$message_type":"artifact","artifact":"/tmp/clis-vixargs_0-1-0/release/deps/libclap_derive-fcea659dae5440c4.so","emit":"link"}"#,
        r#"#47 1.714 {"$message_type":"diagnostic","message":"2 warnings emitted","code":null,"level":"warning","spans":[],"children":[],"rendered":"warning: 2 warnings emitted\n\n"}"#,
        r#"#47 1.714 hi!"#,
    ].into_iter().map(lift_stdio);
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
