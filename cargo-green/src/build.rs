use std::{
    collections::BTreeMap,
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
use indexmap::IndexSet;
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
        self.build(containerfile, target, &[].into(), None).await.3
    }

    pub(crate) async fn build_out(
        &self,
        containerfile: &Utf8Path,
        target: &Stage,
        contexts: &IndexSet<BuildContext>,
        out_dir: &Utf8Path,
    ) -> (String, String, Effects, Result<()>) {
        self.build(containerfile, target, contexts, Some(out_dir)).await
    }

    async fn build(
        &self,
        containerfile: &Utf8Path,
        target: &Stage,
        contexts: &IndexSet<BuildContext>,
        out_dir: Option<&Utf8Path>,
    ) -> (String, String, Effects, Result<()>) {
        let rtrn = |e, effects| ("".to_owned(), "".to_owned(), effects, Err(e));

        assert!(!self.runner.is_none(), "build() called with Runner::None");
        let mut cmd = match self.cmd() {
            Ok(cmd) => cmd,
            Err(e) => return rtrn(e, Effects::default()),
        };
        cmd.arg("build");

        let (call, envs) =
            self.with_docker_args(&mut cmd, containerfile, target, contexts, out_dir);

        let mut effects = Effects::default();
        let status =
            match self.run_build(&mut effects, cmd, &call, containerfile, target, out_dir).await {
                Ok(status) => status,
                Err(e) => return rtrn(e, effects),
            };

        // Something is very wrong here. Try to be helpful by logging some info about runner config:
        if !status.success() {
            let e = effects.try_to_help(&self.runner, self.cargo_home.as_str());
            return rtrn(e, effects);
        }

        (call, envs, effects, Ok(()))
    }

    fn with_docker_args(
        &self,
        cmd: &mut Command,
        containerfile: &Utf8Path,
        target: &Stage,
        contexts: &IndexSet<BuildContext>,
        out_dir: Option<&Utf8Path>,
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
    ) -> Result<ExitStatus> {
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

        let handles = if let Some(out_dir) = out_dir {
            let dbg_out = spawn({
                let target = target.to_owned();
                let out_dir = out_dir.to_owned();
                let cargo_home = self.cargo_home.to_string();
                let stdout = child.stdout.take().expect("started");
                async move { build_stdout(stdout, target, out_dir, cargo_home).await }
            });

            let dbg_err = spawn({
                let stderr = child.stderr.take().expect("started");
                async move { build_stderr(stderr, Some(tx_err)).await }
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
        info!("build ran for {secs:?}");

        if let Ok(e) = rx_err.try_recv() {
            bail!("Runner BUG: {e}")
        }

        // NOTE:
        // * if call to rustc fails, errcode file will exist but the build will complete.
        // * if the call doesn't fail, the file isn't created.
        // * if the build fails that's a bug, and no files will be outputed.

        let Some((dbg_out, dbg_err)) = handles else { return Ok(status) };
        const SOME_TIME: Duration = Duration::from_mins(30);
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
            (Ok(Ok(Ok((out_buf, err_buf, errcode, written)))), _) => {
                let FromStdout { stdout, rustc_envs } = fwd_stdout(&out_buf, "➤", &self.cargo_home);
                info!("Buildscript {PKG}-specific config: envs:{}", rustc_envs.len());
                effects.cargo_rustc_env = rustc_envs;

                let FromStderr { stderr, envs, libs } = fwd_stderr(&err_buf, "✖", &self.cargo_home);
                info!("Suggested {PKG}-specific config: envs:{} libs:{}", envs.len(), libs.len());
                effects.stdout = stdout;
                effects.stderr = stderr;
                effects.written = written;

                Ok(errcode.map(ExitStatus::from_raw).unwrap_or(status))
            }
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Effects {
    pub(crate) written: Vec<Utf8PathBuf>,
    pub(crate) stdout: Vec<String>,
    pub(crate) stderr: Vec<String>,
    pub(crate) cargo_rustc_env: IndexSet<String>,
}

impl Effects {
    fn try_to_help(&self, runner: &Runner, cargo_home: &str) -> Error {
        let rewrite = |msg: &str| {
            let msg = un_virtual_target_dir_str(msg);
            un_rewrite_cargo_home(&msg, cargo_home)
        };

        let cargo_warnings = self
            .stdout
            .iter()
            .filter_map(|line| {
                // https://doc.rust-lang.org/cargo/reference/build-scripts.html#cargo-warning
                line.split_once("cargo:warning=")
                    .xor(line.split_once("cargo::warning="))
                    .map(|(_, rhs)| rhs)
            })
            .map(rewrite)
            .collect::<Vec<_>>()
            .join("\n");

        let cargo_errors = self
            .stdout
            .iter()
            .filter_map(|line| {
                // https://doc.rust-lang.org/cargo/reference/build-scripts.html#cargo-error
                line.split_once("cargo:error=")
                    .xor(line.split_once("cargo::error="))
                    .map(|(_, rhs)| rhs)
            })
            .map(rewrite)
            .collect::<Vec<_>>()
            .join("\n");

        if !cargo_warnings.is_empty() || !cargo_errors.is_empty() {
            return anyhow!("Runner failed.\n{cargo_warnings}\n{cargo_errors}\n");
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
    cargo_home: String,
) -> Result<(String, String, Option<i32>, Vec<Utf8PathBuf>)> {
    info!("running untar on STDOUT");
    let mut buf = Vec::new();
    BufReader::new(stdout)
        .read_to_end(&mut buf)
        .await
        .map_err(|e| anyhow!("Failed getting all the buffer: {e}"))?;
    debug!("produced {target} {}B 0x{}", buf.len(), sha256::digest(&buf));
    let out = BufReader::new(buf.as_slice());

    let mut err_handle = String::new();
    let mut out_handle = String::new();
    let mut rcd = None;
    let mut written = vec![];

    let mut ar = TarArchive::new(out);
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
                write_build_artifact(header, &cargo_home, fname, buf, &f)?;
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

    assert_eq!(header.uid().unwrap(), 0);
    assert_eq!(header.gid().unwrap(), 0);
    assert_eq!(header.mtime().unwrap(), SOURCE_DATE_EPOCH);
    assert_eq!(header.username(), Ok(Some("")));
    assert_eq!(header.groupname(), Ok(Some("")));

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
    rustc_envs: IndexSet<String>,
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
                    if let Some((var, _)) = rhs.split_once("=") {
                        // NOTE: cargo errors if second '=' doesn't exist
                        acc.rustc_envs.insert(var.to_owned().to_owned());
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
