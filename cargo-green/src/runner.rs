use std::{
    collections::BTreeSet,
    env, fmt,
    fs::{self, OpenOptions},
    io::prelude::*,
    mem,
    process::{Output, Stdio},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Result};
use camino::Utf8Path;
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
    extensions::ShowCmd,
    green::Green,
    image_uri::ImageUri,
    logging::{crate_type_for_logging, maybe_log, ENV_LOG_PATH},
    md::BuildContext,
    stage::Stage,
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
        let runner = serde_json::to_string(self).unwrap();
        write!(f, "{}", &runner[1..(runner.len() - 1)])
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
        let network = serde_json::to_string(self).unwrap();
        write!(f, "{}", &network[1..(network.len() - 1)])
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

pub(crate) async fn build_cacheonly(
    green: &Green,
    dockerfile_path: &Utf8Path,
    target: Stage,
) -> Result<()> {
    build(green, dockerfile_path, target, &[].into(), None).await
}

pub(crate) async fn build(
    green: &Green,
    dockerfile_path: &Utf8Path,
    target: Stage,
    contexts: &BTreeSet<BuildContext>,
    out_dir: Option<&Utf8Path>,
) -> Result<()> {
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
    }

    //     cmd.arg(format!("--cache-to=type=registry,ref={img},mode=max,compression=zstd,force-compression=true,oci-mediatypes=true"));
    // // [2024-04-09T07:55:39Z DEBUG lib-autocfg-72217d8ded4d7ec7@177912] ✖ ERROR: Cache export is not supported for the docker driver.
    // // [2024-04-09T07:55:39Z DEBUG lib-autocfg-72217d8ded4d7ec7@177912] ✖ Switch to a different driver, or turn on the containerd image store, and try again.
    // // [2024-04-09T07:55:39Z DEBUG lib-autocfg-72217d8ded4d7ec7@177912] ✖ Learn more at https://docs.docker.com/go/build-cache-backends/
    //TODO: experiment --cache-to=type=inline => try ,mode=max

    //TODO: include --target platform in image tag

    if !green.cache_images.is_empty() {
        for img in &green.cache_images {
            let img = img.noscheme();
            let mode = if false { ",mode=max" } else { "" }; // TODO: env? builder call?
            cmd.arg(format!("--cache-from=type=registry,ref={img}{mode}"));

            let tag = target.as_str(); // TODO: include enough info for repro
                                       // => rustc shortcommit, ..?
                                       // Can buildx give list of all inputs? || short hash(dockerfile + call + envs)
            cmd.arg(format!("--tag={img}:{tag}"));

            let b = crate_type_for_logging("bin").to_ascii_lowercase();
            if [format!("cwd-{b}-"), format!("dep-{b}-")]
                .iter()
                .any(|prefix| tag.starts_with(prefix))
            {
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

    if let Some((out_task, err_task)) = handles {
        let longish = Duration::from_secs(2);
        match join!(timeout(longish, out_task), timeout(longish, err_task)) {
            (Ok(Ok(Ok(_))), Ok(Ok(Ok(written)))) => {
                if !written.is_empty() {
                    info!("rustc wrote {} files:", written.len());
                    for f in written {
                        let f = Utf8Path::new(&f);
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

    if let Some(path) = green.final_path.as_deref() {
        info!("Writing final Dockerfile to {path}");

        //TODO: only write if final root pkg? => how to detect? parse cargo args?(:/)
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

    Ok(())
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
) -> JoinHandle<Result<Vec<String>>>
where
    R: AsyncBufRead + Unpin + Send + 'static,
{
    debug!("Starting {name} task {badge}");
    let start = Instant::now();
    let fwder = if mark == MARK_STDOUT { fwd_stdout } else { fwd_stderr };
    spawn(async move {
        let mut buf = String::new();
        let mut details: Vec<String> = vec![];
        let mut written: Vec<String> = vec![];
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
                fwder(msg, &mut buf, &mut written);
            }

            // //warning: panic message contains an unused formatting placeholder
            // //--> /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/proc-macro2-1.0.36/build.rs:191:17
            // => un-rewrite /index.crates.io-0000000000000000/ in cargo messages
            // => also in .d files
            // cache should be ok (cargo's point of view) if written right after green's build(..) call

            // Show data transfers (Bytes, maybe also timings?)
            let pattern = " transferring ";
            for (idx, _pattern) in line.as_str().match_indices(pattern) {
                let detail = line[(pattern.len() + idx)..].trim_end_matches(" done");
                details.push(detail.to_owned());
            }

            //TODO: count DONEs and CACHEDs
            // D 25/03/31 23:21:11.702 L winnow 0.7.3-2a24a5220012bbd8 ✖ #0 building with "default" instance using docker driver                                                           01:21:12 [46/1876]
            // D 25/03/31 23:21:11.702 L winnow 0.7.3-2a24a5220012bbd8 ✖ #1 [internal] load build definition from Dockerfile
            // D 25/03/31 23:21:11.702 L winnow 0.7.3-2a24a5220012bbd8 ✖ #1 transferring dockerfile: 6.92kB done
            // D 25/03/31 23:21:11.702 L winnow 0.7.3-2a24a5220012bbd8 ✖ #1 DONE 0.0s
            // D 25/03/31 23:21:11.702 L winnow 0.7.3-2a24a5220012bbd8 ✖ #2 resolve image config for docker-image://docker.io/docker/dockerfile:1@sha256:4c68376a702446fc3c79af22de146a148bc3367e73c25a5803d453b6b3f722fb
            // D 25/03/31 23:21:11.702 L winnow 0.7.3-2a24a5220012bbd8 ✖ #2 DONE 0.0s
            // D 25/03/31 23:21:11.702 L winnow 0.7.3-2a24a5220012bbd8 ✖ #3 docker-image://docker.io/docker/dockerfile:1@sha256:4c68376a702446fc3c79af22de146a148bc3367e73c25a5803d453b6b3f722fb
            // D 25/03/31 23:21:11.702 L winnow 0.7.3-2a24a5220012bbd8 ✖ #3 CACHED
            // D 25/03/31 23:21:11.702 L winnow 0.7.3-2a24a5220012bbd8 ✖ #4 [internal] load metadata for docker.io/library/rust:1.85.0-slim@sha256:1829c432be4a592f3021501334d3fcca24f238432b13306a4e62669dec538e52
            // D 25/03/31 23:21:11.852 L winnow 0.7.3-2a24a5220012bbd8 ✖ #4 DONE 0.0s
            // D 25/03/31 23:21:11.852 L winnow 0.7.3-2a24a5220012bbd8 ✖ #5 [internal] load metadata for docker.io/tonistiigi/xx:latest
            // D 25/03/31 23:21:11.852 L winnow 0.7.3-2a24a5220012bbd8 ✖ #5 DONE 0.0s
            // D 25/03/31 23:21:11.852 L winnow 0.7.3-2a24a5220012bbd8 ✖ #6 [internal] load .dockerignore
            // D 25/03/31 23:21:11.852 L winnow 0.7.3-2a24a5220012bbd8 ✖ #6 transferring context: 2B done
            // D 25/03/31 23:21:11.852 L winnow 0.7.3-2a24a5220012bbd8 ✖ #6 DONE 0.0s
            // D 25/03/31 23:21:11.852 L winnow 0.7.3-2a24a5220012bbd8 ✖ #7 [xx 1/1] FROM docker.io/tonistiigi/xx:latest
            // D 25/03/31 23:21:11.852 L winnow 0.7.3-2a24a5220012bbd8 ✖ #7 DONE 0.0s
            // D 25/03/31 23:21:11.852 L winnow 0.7.3-2a24a5220012bbd8 ✖ #8 [rust-base 1/2] FROM docker.io/library/rust:1.85.0-slim@sha256:1829c432be4a592f3021501334d3fcca24f238432b13306a4e62669dec538e52
            // D 25/03/31 23:21:11.852 L winnow 0.7.3-2a24a5220012bbd8 ✖ #8 DONE 0.0s
            // D 25/03/31 23:21:11.852 L winnow 0.7.3-2a24a5220012bbd8 ✖ #9 [dep-l-winnow-0.7.3-2a24a5220012bbd8 1/3] WORKDIR /tmp/clis-cargo-udeps_0-1-55/release/deps
            // D 25/03/31 23:21:11.852 L winnow 0.7.3-2a24a5220012bbd8 ✖ #9 CACHED
            // D 25/03/31 23:21:11.852 L winnow 0.7.3-2a24a5220012bbd8 ✖ #10 [cratesio-winnow-0.7.3 1/2] ADD --chmod=0664 --checksum=sha256:0e7f4ea97f6f78012141bcdb6a216b2609f0979ada50b20ca5b52dde2eac2bb1   https://static.crates.io/crates/winnow/winnow-0.7.3.crate /crate
            // D 25/03/31 23:21:11.852 L winnow 0.7.3-2a24a5220012bbd8 ✖ #10 CACHED

            // D 25/03/30 03:30:33.891 L mime_guess 2.0.5-d2ac06fbe540be6e ✖ #30 0.265 ::STDERR:: {"$message_type":"diagnostic","message":"environment variable `MIME_TYPES_GENERATED_PATH` not defined at co
            // mpile time","code":null,"level":"error","spans":[{"file_name":"/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/mime_guess-2.0.5/src/impl_bin_search.rs","byte_start":62,"byte_
            // end":95,"line_start":4,"line_end":4,"column_start":10,"column_end":43,"is_primary":true,"text":[{"text":"include!(env!(\"MIME_TYPES_GENERATED_PATH\"));","highlight_start":10,"highlight_end":
            // 43}],"label":null,"suggested_replacement":null,"suggestion_applicability":null,"expansion":{"span":{"file_name":"/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/mime_guess-2.
            // 0.5/src/impl_bin_search.rs","byte_start":62,"byte_end":95,"line_start":4,"line_end":4,"column_start":10,"column_end":43,"is_primary":false,"text":[{"text":"include!(env!(\"MIME_TYPES_GENERAT
            // ED_PATH\"));","highlight_start":10,"highlight_end":43}],"label":null,"suggested_replacement":null,"suggestion_applicability":null,"expansion":null},"macro_decl_name":"env!","def_site_span":{
            // "file_name":"/rustc/4d91de4e48198da2e33413efdcd9cd2cc0c46688/library/core/src/macros/mod.rs","byte_start":38805,"byte_end":38821,"line_start":1101,"line_end":1101,"column_start":5,"column_en
            // d":21,"is_primary":false,"text":[],"label":null,"suggested_replacement":null,"suggestion_applicability":null,"expansion":null}}}],"children":[{"message":"use `std::env::var(\"MIME_TYPES_GENERATED_PATH\")` to read the variable at run time","code":null,"level":"help","spans":[],"children":[],"rendered":null}],"rendered":"error: environment variable `MIME_TYPES_GENERATED_PATH` not
            //  defined at compile time\n --> /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/mime_guess-2.0.5/src/impl_bin_search.rs:4:10\n  |\n4 | include!(env!(\"MIME_TYPES_GENERATED_PAT
            // H\"));\n  |          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  |\n  = help: use `std::env::var(\"MIME_TYPES_GENERATED_PATH\")` to read the variable at run time\n  = note: this error originates in
            //  the macro `env` (in Nightly builds, run with -Z macro-backtrace for more info)\n\n"}
            // D 25/03/30 03:30:33.891 L mime_guess 2.0.5-d2ac06fbe540be6e ✖ #30 0.265 ::STDERR:: {"$message_type":"artifact","artifact":"/tmp/clis-torrust-index_3-0-0-alpha-12/release/deps/mime_guess-d2ac
            // 06fbe540be6e.d","emit":"dep-info"}
            // I 25/03/30 03:30:33.891 L mime_guess 2.0.5-d2ac06fbe540be6e rustc wrote /tmp/clis-torrust-index_3-0-0-alpha-12/release/deps/mime_guess-d2ac06fbe540be6e.d
            // D 25/03/30 03:30:33.891 L mime_guess 2.0.5-d2ac06fbe540be6e ✖ #30 0.265 ::STDERR::
            // g=-fuse-ld=/usr/local/bin/mold`                                                                              error: environment variable `MIME_TYPES_GENERATED_PATH` not defined at compile time                                                                                                            --> /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/mime_guess-2.0.5/src/impl_bin_search.rs:4:10                                                                               |
            // 4 | include!(env!("MIME_TYPES_GENERATED_PATH"));
            //   |          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
            //   |
            //   = help: use `std::env::var("MIME_TYPES_GENERATED_PATH")` to read the variable at run time
            //   = note: this error originates in the macro `env` (in Nightly builds, run with -Z macro-backtrace for more info)
            // => TODO: suggest    CARGOGREEN_SET_ENVS='MIME_TYPES_GENERATED_PATH RING_CORE_PREFIX'

            // I 25/03/31 22:33:29.519 X cargo 0.81.0-d28c49a0e79c24d1 rustc wrote /tmp/clis-cargo-udeps_0-1-50/release/build/cargo-d28c49a0e79c24d1/build_script_build-d28c49a0e79c24d1.d
            // D 25/03/31 22:33:29.665 X cargo 0.81.0-d28c49a0e79c24d1 ✖ #55 0.310 ::STDERR:: {"$message_type":"diagnostic","message":"linking with `cc` failed: exit status: 1","code":null,"level":"error",
            // "spans":[],"children":[{"message":"LC_ALL=\"C\" PATH=\"/usr/local/rustup/toolchains/1.85.0-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/bin:/usr/local/cargo/bin:/usr/local/s
            // bin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin\" VSLANG=\"1033\" \"cc\" \"-m64\" \"/tmp/rustcaqBYLY/symbols.o\" \"<5 object files omitted>\" \"-Wl,--as-needed\" \"-Wl,-Bstatic\" \"/tmp/cli
            // s-cargo-udeps_0-1-50/release/deps/{libtar-abaf872bda85c202.rlib,libfiletime-fe7a973ce74b2e13.rlib,liblibc-bea4c89311b7f903.rlib,libflate2-949eb855661d0057.rlib,liblibz_sys-7c0f9d8ec45388e2.r
            // lib,libcrc32fast-df9d3a277d606584.rlib,libcfg_if-902bdc8a7a06a77f.rlib}\" \"/usr/local/rustup/toolchains/1.85.0-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-6273
            // 572f18644c87.rlib,libpanic_unwind-267e668abf74a283.rlib,libobject-ec6154ccae37a33e.rlib,libmemchr-500edd5521c440d4.rlib,libaddr2line-86d8d9428792e8ef.rlib,libgimli-10f06487503767c2.rlib,libr
            // ustc_demangle-6a38424de1e5bca5.rlib,libstd_detect-de9763ea1c19dca3.rlib,libhashbrown-a7f5bb2f736d3c49.rlib,librustc_std_workspace_alloc-7e368919bdc4a44c.rlib,libminiz_oxide-376454d49910c786.
            // rlib,libadler-fa99f5692b5dce85.rlib,libunwind-91cafdaf16f7fe40.rlib,libcfg_if-f7ee3f1ea78d9dae.rlib,liblibc-d3a35665f881365a.rlib,liballoc-715bc629a88bca60.rlib,librustc_std_workspace_core-a
            // e70165d1278cff7.rlib,libcore-406129d0e3fbc101.rlib,libcompiler_builtins-1af05515ab19524a.rlib}\" \"-Wl,-Bdynamic\" \"-lz\" \"-lgcc_s\" \"-lutil\" \"-lrt\" \"-lpthread\" \"-lm\" \"-ldl\" \"-l
            // c\" \"-Wl,--eh-frame-hdr\" \"-Wl,-z,noexecstack\" \"-L\" \"/usr/local/rustup/toolchains/1.85.0-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/lib\" \"-o\" \"/tmp/clis-cargo-ud
            // eps_0-1-50/release/build/cargo-d28c49a0e79c24d1/build_script_build-d28c49a0e79c24d1\" \"-Wl,--gc-sections\" \"-pie\" \"-Wl,-z,relro,-z,now\" \"-Wl,--strip-all\" \"-nodefaultlibs\"","code":nu
            // ll,"level":"note","spans":[],"children":[],"rendered":null},{"message":"some arguments are omitted. use `--verbose` to show all linker arguments","code":null,"level":"note","spans":[],"child
            // ren":[],"rendered":null},{"message":"/usr/bin/ld: cannot find -lz: No such file or directory\ncollect2: error: ld returned 1 exit status\n","code":null,"level":"note","spans":[],"children":[
            // ],"rendered":null}],"rendered":"error: linking with `cc` failed: exit status: 1\n  |\n  = note: LC_ALL=\"C\" PATH=\"/usr/local/rustup/toolchains/1.85.0-x86_64-unknown-linux-gnu/lib/rustlib/x
            // 86_64-unknown-linux-gnu/bin:/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin\" VSLANG=\"1033\" \"cc\" \"-m64\" \"/tmp/rustcaqBYLY/symbols.o\" \"<5 object fil
            // es omitted>\" \"-Wl,--as-needed\" \"-Wl,-Bstatic\" \"/tmp/clis-cargo-udeps_0-1-50/release/deps/{libtar-abaf872bda85c202.rlib,libfiletime-fe7a973ce74b2e13.rlib,liblibc-bea4c89311b7f903.rlib,l
            // ibflate2-949eb855661d0057.rlib,liblibz_sys-7c0f9d8ec45388e2.rlib,libcrc32fast-df9d3a277d606584.rlib,libcfg_if-902bdc8a7a06a77f.rlib}\" \"/usr/local/rustup/toolchains/1.85.0-x86_64-unknown-li
            // nux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-6273572f18644c87.rlib,libpanic_unwind-267e668abf74a283.rlib,libobject-ec6154ccae37a33e.rlib,libmemchr-500edd5521c440d4.rlib,libaddr2l
            // ine-86d8d9428792e8ef.rlib,libgimli-10f06487503767c2.rlib,librustc_demangle-6a38424de1e5bca5.rlib,libstd_detect-de9763ea1c19dca3.rlib,libhashbrown-a7f5bb2f736d3c49.rlib,librustc_std_workspace
            // _alloc-7e368919bdc4a44c.rlib,libminiz_oxide-376454d49910c786.rlib,libadler-fa99f5692b5dce85.rlib,libunwind-91cafdaf16f7fe40.rlib,libcfg_if-f7ee3f1ea78d9dae.rlib,liblibc-d3a35665f881365a.rlib
            // ,liballoc-715bc629a88bca60.rlib,librustc_std_workspace_core-ae70165d1278cff7.rlib,libcore-406129d0e3fbc101.rlib,libcompiler_builtins-1af05515ab19524a.rlib}\" \"-Wl,-Bdynamic\" \"-lz\" \"-lgc
            // c_s\" \"-lutil\" \"-lrt\" \"-lpthread\" \"-lm\" \"-ldl\" \"-lc\" \"-Wl,--eh-frame-hdr\" \"-Wl,-z,noexecstack\" \"-L\" \"/usr/local/rustup/toolchains/1.85.0-x86_64-unknown-linux-gnu/lib/rustl
            // ib/x86_64-unknown-linux-gnu/lib\" \"-o\" \"/tmp/clis-cargo-udeps_0-1-50/release/build/cargo-d28c49a0e79c24d1/build_script_build-d28c49a0e79c24d1\" \"-Wl,--gc-sections\" \"-pie\" \"-Wl,-z,rel
            // ro,-z,now\" \"-Wl,--strip-all\" \"-nodefaultlibs\"\n  = note: some arguments are omitted. use `--verbose` to show all linker arguments\n  = note: /usr/bin/ld: cannot find -lz: No such file o
            // r directory\n          collect2: error: ld returned 1 exit status\n          \n\n"}
            // D 25/03/31 22:33:29.665 X cargo 0.81.0-d28c49a0e79c24d1 ✖ #55 0.326 ::STDERR:: {"$message_type":"diagnostic","message":"aborting due to 1 previous error","code":null,"level":"error","spans":
            // [],"children":[],"rendered":"error: aborting due to 1 previous error\n\n"}
            // error: linking with `cc` failed: exit status: 1
            //   |
            //   = note: LC_ALL="C" PATH="/usr/local/rustup/toolchains/1.85.0-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/bin:/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin
            // :/usr/bin:/sbin:/bin" VSLANG="1033" "cc" "-m64" "/tmp/rustcaqBYLY/symbols.o" "<5 object files omitted>" "-Wl,--as-needed" "-Wl,-Bstatic" "/tmp/clis-cargo-udeps_0-1-50/release/deps/{libtar-ab
            // af872bda85c202.rlib,libfiletime-fe7a973ce74b2e13.rlib,liblibc-bea4c89311b7f903.rlib,libflate2-949eb855661d0057.rlib,liblibz_sys-7c0f9d8ec45388e2.rlib,libcrc32fast-df9d3a277d606584.rlib,libcf
            // g_if-902bdc8a7a06a77f.rlib}" "/usr/local/rustup/toolchains/1.85.0-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-6273572f18644c87.rlib,libpanic_unwind-267e668abf74
            // a283.rlib,libobject-ec6154ccae37a33e.rlib,libmemchr-500edd5521c440d4.rlib,libaddr2line-86d8d9428792e8ef.rlib,libgimli-10f06487503767c2.rlib,librustc_demangle-6a38424de1e5bca5.rlib,libstd_det
            // ect-de9763ea1c19dca3.rlib,libhashbrown-a7f5bb2f736d3c49.rlib,librustc_std_workspace_alloc-7e368919bdc4a44c.rlib,libminiz_oxide-376454d49910c786.rlib,libadler-fa99f5692b5dce85.rlib,libunwind-
            // 91cafdaf16f7fe40.rlib,libcfg_if-f7ee3f1ea78d9dae.rlib,liblibc-d3a35665f881365a.rlib,liballoc-715bc629a88bca60.rlib,librustc_std_workspace_core-ae70165d1278cff7.rlib,libcore-406129d0e3fbc101.
            // rlib,libcompiler_builtins-1af05515ab19524a.rlib}" "-Wl,-Bdynamic" "-lz" "-lgcc_s" "-lutil" "-lrt" "-lpthread" "-lm" "-ldl" "-lc" "-Wl,--eh-frame-hdr" "-Wl,-z,noexecstack" "-L" "/usr/local/ru
            // stup/toolchains/1.85.0-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/lib" "-o" "/tmp/clis-cargo-udeps_0-1-50/release/build/cargo-d28c49a0e79c24d1/build_script_build-d28c49a0e
            // 79c24d1" "-Wl,--gc-sections" "-pie" "-Wl,-z,relro,-z,now" "-Wl,--strip-all" "-nodefaultlibs"
            //   = note: some arguments are omitted. use `--verbose` to show all linker arguments
            //   = note: /usr/bin/ld: cannot find -lz: No such file or directory
            //           collect2: error: ld returned 1 exit status
            // => suggest "-lz":
            // [package.metadata.green]
            // add.apt = [ "zlib1g-dev" ]
        }
        debug!("Terminating {name} task {details:?}");
        drop(stdio);
        Ok(written)
    })
}

#[test]
fn support_long_broken_json_lines() {
    let logs = assertx::setup_logging_test();
    let lines = [
        r#"#42 1.312 ::STDERR:: {"$message_type":"artifact","artifact":"/tmp/thing","emit":"link""#,
        r#"#42 1.313 ::STDERR:: }"#,
    ];
    let mut buf = String::new();
    let mut written = vec![];

    let msg = lift_stdio(lines[0], MARK_STDERR);
    assert_eq!(msg, Some(r#"{"$message_type":"artifact","artifact":"/tmp/thing","emit":"link""#));
    fwd_stderr(msg.unwrap(), &mut buf, &mut written);
    assert_eq!(buf, r#"{"$message_type":"artifact","artifact":"/tmp/thing","emit":"link""#);
    assert_eq!(written, Vec::<String>::new());

    let msg = lift_stdio(lines[1], MARK_STDERR);
    assert_eq!(msg, Some("}"));
    fwd_stderr(msg.unwrap(), &mut buf, &mut written);
    assert_eq!(buf, "");
    assert_eq!(written, vec!["/tmp/thing".to_owned()]);

    // Then fwd_stderr
    // calls artifact_written(r#"{"$message_type":"artifact","artifact":"/tmp/thing","emit":"link"}"#)
    // which returns Some("/tmp/thing")
    assertx::assert_logs_contain_in_order!(logs, log::Level::Info => "rustc wrote /tmp/thing");
}

fn fwd_stderr(msg: &str, buf: &mut String, written: &mut Vec<String>) {
    let mut show = |msg: &str| {
        info!("(To cargo's STDERR): {msg}");
        eprintln!("{msg}");

        if let Some(file) = artifact_written(msg) {
            written.push(file.to_owned());
            info!("rustc wrote {file}");
        }
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

fn fwd_stdout(
    msg: &str,
    #[expect(clippy::ptr_arg)] _buf: &mut String,
    #[expect(clippy::ptr_arg)] _written: &mut Vec<String>,
) {
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

// Maybe replace with actual JSON deserialization
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
fn lift_stdio<'a>(line: &'a str, mark: &'static str) -> Option<&'a str> {
    // Docker builds running shell code usually start like: #47 0.057
    let line = line.trim_start_matches(|c| ['#', '.', ' '].contains(&c) || c.is_ascii_digit());
    let msg = line.trim_start_matches(mark);
    let cut = msg.len() != line.len();
    let msg = msg.trim();
    (cut && !msg.is_empty()).then_some(msg)
}
