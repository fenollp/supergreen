//! Runs builds against a bare `buildkitd` daemon through `buildctl`, BuildKit's own client.
//!
//! This is the [`crate::runner::Runner::BuildKit`] counterpart to the `docker buildx build` /
//! `podman build` path implemented in [`crate::build`]. The daemon is addressed through
//! `$BUILDKIT_HOST` (defaults to `unix:///run/buildkit/buildkitd.sock`) and the client binary
//! resolves through `$CARGOGREEN_BUILDCTL`, falling back to `buildctl` in `$PATH`. e.g:
//!
//! ```console
//! docker buildx build --platform=local --target=binaries -o=bin \
//!   --build-arg=BUILDKIT_CONTEXT_KEEP_GIT_DIR=1 https://github.com/moby/buildkit.git#v0.31
//! export CARGOGREEN_BUILDCTL=$PWD/bin/buildctl
//! export CARGOGREEN_RUNNER=buildkit
//! ```
//!
//! # Why shell out to `buildctl` instead of using a Rust BuildKit client crate?
//!
//! As of 2026-07 no crate implements enough of the client side of BuildKit's gRPC + session
//! protocol for what's needed here. Closest candidate is `buildkit-client` v0.1.5 (also published
//! as `bkit`, <https://lib.rs/crates/buildkit-client>): a pure-Rust tonic-based client that does
//! implement `Control.Solve`/`Status` and the bidirectional session (filesync DiffCopy, auth,
//! secrets). It is however missing all of:
//! * the `tar` exporter: receiving the output tarball requires the client to also serve
//!   `moby.filesync.v1.FileSend` over the session (the daemon calls back into the client)
//! * the `local` cache import/export backends (export also goes through `FileSend`)
//! * named build contexts (the `context:<name>` frontend attr isn't exposed)
//! * non-TCP endpoints: no `unix://`, and no `docker-container://` (buildx-managed builders)
//!
//! `buildkit-rs` (cicadahq) and `buildkit-llb`/`buildkit-frontend` (denzp) target LLB/frontend
//! authorship, not a full `build`-with-exporters client, and are unmaintained.
//!
//! Once some crate (or a homegrown `FileSend` service on top of `buildkit-client`'s session)
//! covers the tar exporter, this module is the seam where process spawning gets replaced.

use std::{fs, process::Stdio};

use anyhow::{Result, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexSet;
use log::info;
use tokio::{join, process::Command};

use crate::{
    PKG, build::Effects, cache::result::ResultWriter, ext::CommandExt, green::Green,
    md::BuildContext, network::Network, retrier::Retrier, stage::Stage,
};

impl Green {
    pub(crate) async fn buildctl_build_cacheonly(
        &self,
        containerfile: &Utf8Path,
        target: &Stage,
    ) -> Result<()> {
        let contexts = [].into();
        // TODO: ^C handling that kills both builds (and retries)
        let (_tui, matched) = join!(
            self.buildctl_build(containerfile, target, &contexts, None, true),
            self.buildctl_build(containerfile, target, &contexts, None, false),
        );
        matched.4
    }

    pub(crate) async fn buildctl_build_out(
        &self,
        containerfile: &Utf8Path,
        target: &Stage,
        contexts: &IndexSet<BuildContext>,
        out_dir: &Utf8Path,
    ) -> (String, String, Effects, Option<ResultWriter>, Result<()>) {
        // Contrary to the buildx path, no concurrent cache-export build is needed:
        // a single buildctl call handles both the tar output and any cache exports.
        self.buildctl_build(containerfile, target, contexts, Some(out_dir), false).await
    }

    async fn buildctl_build(
        &self,
        containerfile: &Utf8Path,
        target: &Stage,
        contexts: &IndexSet<BuildContext>,
        out_dir: Option<&Utf8Path>,
        tui: bool,
    ) -> (String, String, Effects, Option<ResultWriter>, Result<()>) {
        assert!(self.runner.is_buildctl(), "buildctl_build() called with runner {}", self.runner);

        let mut retrier = Retrier::with_max_attempts(5);
        loop {
            let fail = |e| ("".to_owned(), "".to_owned(), Effects::default(), None, Err(e));

            let mut cmd = match self.buildctl_cmd() {
                Ok(cmd) => cmd,
                Err(e) => return fail(e),
            };

            let (call, envs) = match self.with_buildctl_args(
                &mut cmd,
                containerfile,
                target,
                contexts,
                out_dir,
                tui,
            ) {
                Ok(call_envs) => call_envs,
                Err(e) => return fail(e),
            };

            let mut effects = Effects::default();
            let (status, result) = match self
                .run_build(&mut effects, cmd, &call, containerfile, target, out_dir, tui)
                .await
            {
                Ok((status, result)) => (status, result),
                Err(e) => return (call, envs, effects, None, Err(e)),
            };

            // Something is very wrong here. Try to be helpful by logging some info about runner config:
            if !status.success() {
                let (retryme, e) = effects.try_to_help(&self.runner, self.cargo_home.as_str());
                if retryme && retrier.continues() {
                    retrier.backoff("build", e).await;
                    continue;
                }
                let e = anyhow!("retried {} times: {e}", retrier.max());
                return (call, envs, effects, result, Err(e));
            }

            return (call, envs, effects, result, Ok(()));
        }
    }

    fn buildctl_cmd(&self) -> Result<Command> {
        let mut cmd = Command::new(self.runner.executable()?);
        cmd.kill_on_drop(true); // Underlying OS process dies with us
        cmd.stdin(Stdio::null()); // The containerfile goes through a synced dir, not STDIN
        cmd.env_clear(); // Pass all envs explicitly only

        // buildctl only cares about $BUILDKIT_HOST (and $PATH), other runner envs are inert.
        for (var, val) in &self.runner_envs {
            info!("passing through runner setting: ${var}={val:?}");
            cmd.env(var, val);
        }

        Ok(cmd)
    }

    fn with_buildctl_args(
        &self,
        cmd: &mut Command,
        containerfile: &Utf8Path,
        target: &Stage,
        contexts: &IndexSet<BuildContext>,
        out_dir: Option<&Utf8Path>,
        tui: bool,
    ) -> Result<(String, String)> {
        cmd.arg("build");

        // The builtin frontend still honors any `# syntax=` directive in the containerfile.
        cmd.arg("--frontend=dockerfile.v0");

        let dir = containerfile
            .parent()
            .ok_or_else(|| anyhow!("BUG: {containerfile} has no parent dir"))?;
        let file_name = containerfile
            .file_name()
            .ok_or_else(|| anyhow!("BUG: {containerfile} has no file name"))?;
        // Only `filename` gets synced from this local: the frontend asks for that single path.
        cmd.arg(format!("--local=dockerfile={dir}"));
        cmd.arg(format!("--opt=filename={file_name}"));

        // The dockerfile frontend requires a default context. Ours is always empty:
        // stages read inputs from named build contexts and network fetches only.
        cmd.arg(format!("--local=context={}", self.empty_context_dir()?));

        if self.repro() {
            cmd.arg("--no-cache");
        }

        for img in self.cache.from_images.iter().chain(self.cache.images.iter()) {
            let img = img.noscheme();
            cmd.arg(format!("--import-cache=type=registry,ref={img}"));
        }
        for img in self.cache.to_images.iter().chain(self.cache.images.iter()) {
            let img = img.noscheme();
            // A bare buildkitd always supports cache export (cf. `maxready` in the buildx path),
            // and there is no docker image store to `--tag` + `--load` into.
            cmd.arg(format!("--export-cache=type=registry,ref={img},mode=max,ignore-error=false"));
        }

        match self.base.with_network {
            Network::Default => {} // BuildKit's default netmode: sandbox
            Network::None => {
                cmd.arg("--opt=force-network-mode=none");
            }
            Network::Host => {
                // Requires buildkitd started with insecure-entitlements = ["network.host"]
                cmd.arg("--opt=force-network-mode=host");
                cmd.arg("--allow=network.host");
            }
        }

        cmd.arg(format!("--opt=target={target}"));

        if out_dir.is_some() {
            cmd.arg("--output=type=tar"); // No dest set: the tarball streams to STDOUT
        }
        // else: exporting nothing is buildctl's --output=type=cacheonly

        if let Some(ref dirs) = self.dirs
            && self.cachebuildkit()
        {
            if out_dir.is_some()
                && let Some(dst) = dirs.new_runner_cache(target)?
            {
                cmd.arg(format!(
                    "--export-cache=type=local,dest={dst},ignore-error=true,mode=max,oci-mediatypes=true,image-manifest=true,compression=gzip,compression-level=0,force-compression=false"
                ));
            }
            if let Some(src) = dirs.runner_cache(target) {
                cmd.arg(format!("--import-cache=type=local,src={src}"));
            }
        }

        for BuildContext { name, uri } in contexts {
            // buildx's --build-context=name=dir splits into a local + a frontend attr:
            cmd.arg(format!("--local={name}={uri}"));
            cmd.arg(format!("--opt=context:{name}=local:{name}"));
        }

        if out_dir.is_some() {
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());
        } else if !tui {
            cmd.stderr(Stdio::piped()); // tee to Effects.stderr: to try_to_help
        }
        // else: BuildKit's ANSI progress UI

        if !tui {
            cmd.arg("--progress=plain");
        }

        let call = cmd.show();
        let envs = cmd.envs_string(&self.runner.buildnoop_envs());
        if !tui {
            info!("Starting `{envs} {call}`");
            eprintln!("Starting `{envs} {call}`");
        }
        let call = call
            .split_whitespace()
            .filter(|flag| !self.runner.buildnoop_flags().any(|prefix| flag.starts_with(prefix)))
            .filter(|flag| !flag.starts_with("--opt=target="))
            .filter(|flag| !flag.starts_with("--opt=filename="))
            .filter(|flag| !flag.starts_with("--local=")) // Host-specific paths
            .filter(|flag| *flag != "--progress=plain")
            .map(|flag| if flag.starts_with("--output=") { "--output=." } else { flag })
            .collect::<Vec<_>>()
            .join(" ")
            .replace(cmd.as_std().get_program().to_str().unwrap(), &self.runner.to_string());

        Ok((call, envs))
    }

    fn empty_context_dir(&self) -> Result<Utf8PathBuf> {
        let dir = if let Some(ref dirs) = self.dirs {
            dirs.tmp.join("empty-context")
        } else {
            let tmp: Utf8PathBuf = std::env::temp_dir()
                .try_into()
                .map_err(|e| anyhow!("Temp dir path is not utf-8: {e}"))?;
            tmp.join(format!("{PKG}-empty-context"))
        };
        fs::create_dir_all(&dir).map_err(|e| anyhow!("Failed to `mkdir -p {dir}`: {e}"))?;
        Ok(dir)
    }
}
