use std::{
    collections::BTreeSet,
    env, mem,
    process::{Output, Stdio},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info, warn};
use reqwest::Client as ReqwestClient;
use serde::Deserialize;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, BufReader as TokioBufReader, Lines},
    join,
    process::Command,
    spawn,
    task::JoinHandle,
    time::{error::Elapsed, timeout},
};

use crate::{
    envs::{cache_image, runner, runs_on_network},
    extensions::ShowCmd,
    md::BuildContext,
    parse::crate_type_for_logging,
    stage::Stage,
};

pub(crate) const MARK_STDOUT: &str = "::STDOUT:: ";
pub(crate) const MARK_STDERR: &str = "::STDERR:: ";

#[must_use]
pub(crate) fn runner_cmd() -> Command {
    let mut cmd = Command::new(runner());
    cmd.kill_on_drop(true);
    cmd.stdin(Stdio::null());
    cmd.arg("--debug");
    // TODO: use env_clear https://docs.rs/tokio/latest/tokio/process/struct.Command.html#method.env_clear
    cmd
}

#[must_use]
pub(crate) async fn maybe_lock_image(mut img: String) -> String {
    // Lock image, as podman(4.3.1) does not respect --pull=false (fully, anyway)
    if img.starts_with("docker-image://") && !img.contains("@sha256:") {
        if let Some(line) = runner_cmd()
            .arg("inspect")
            .arg("--format={{index .RepoDigests 0}}")
            .arg(img.trim_start_matches("docker-image://"))
            .output()
            .await
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|x| x.lines().next().map(ToOwned::to_owned))
        {
            img.push_str(line.trim_start_matches(|c| c != '@'));
        }
    }
    img
}

pub(crate) async fn fetch_digest(img: &str) -> Result<String> {
    // e.g. docker-image://docker.io/library/rust:1.80.1-slim
    if !img.starts_with("docker-image://") {
        bail!("Image missing 'docker-image' scheme: {img}")
    }
    let img = img.trim_start_matches("docker-image://");
    if img.contains('@') {
        bail!("Image is already locked: {img}")
    }
    let Some((path, tag)) = img.split_once(':') else { bail!("Image is missing a tag: {img}") };
    let path: Utf8PathBuf = path.into();
    let (dir, img) = match path.iter().collect::<Vec<_>>()[..] {
        ["docker.io", dir, img] => (dir, img),
        _ => bail!("BUG: unhandled image path {path}"),
    };

    let txt = ReqwestClient::new()
        .get(format!("https://registry.hub.docker.com/v2/repositories/{dir}/{img}/tags/{tag}"))
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

    Ok(format!("docker-image://{path}:{tag}@{digest}"))
}

pub(crate) async fn build(
    command: &str,
    dockerfile_path: &Utf8Path,
    target: Stage,
    contexts: &BTreeSet<BuildContext>,
    out_dir: &Utf8Path,
) -> Result<Option<i32>> {
    let mut cmd = Command::new(command);
    cmd.arg("--debug");
    cmd.arg("build");

    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());

    // Makes sure the underlying OS process dies with us
    cmd.kill_on_drop(true);

    // Makes sure that the BuildKit builder is used by either runner
    cmd.env("DOCKER_BUILDKIT", "1");

    //TODO: (use if set) cmd.env("SOURCE_DATE_EPOCH", "0"); // https://reproducible-builds.org/docs/source-date-epoch
    // https://github.com/moby/buildkit/blob/master/docs/build-repro.md#source_date_epoch
    // Set SOURCE_DATE_EPOCH=$(git log -1 --pretty=%ct) for local code, and
    // set it to crates' birth date, in case it's a $HOME/.cargo/registry/cache/...crate
    // set it to the directory's birth date otherwise (should be a relative path to local files).

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
    }

    //     cmd.arg(format!("--cache-to=type=registry,ref={img},mode=max,compression=zstd,force-compression=true,oci-mediatypes=true"));
    // // [2024-04-09T07:55:39Z DEBUG lib-autocfg-72217d8ded4d7ec7@177912] ✖ ERROR: Cache export is not supported for the docker driver.
    // // [2024-04-09T07:55:39Z DEBUG lib-autocfg-72217d8ded4d7ec7@177912] ✖ Switch to a different driver, or turn on the containerd image store, and try again.
    // // [2024-04-09T07:55:39Z DEBUG lib-autocfg-72217d8ded4d7ec7@177912] ✖ Learn more at https://docs.docker.com/go/build-cache-backends/

    if let Some(img) = cache_image() {
        let img = img.trim_start_matches("docker-image://");
        cmd.arg(format!(
            "--cache-from=type=registry,ref={img}{}", // TODO: check img for commas
            if false { ",mode=max" } else { "" }      // TODO: env? builder call?
        ));

        let tag = target.to_string(); // TODO: include enough info for repro
                                      // => rustc shortcommit, ..?
                                      // Can buildx give list of all inputs? || short hash(dockerfile + call + envs)
        cmd.arg(format!("--tag={img}:{tag}"));
        if tag.starts_with(&format!("cwd-{}-", crate_type_for_logging("bin"))) {
            cmd.arg(format!("--tag={img}:latest"));
        }
        cmd.arg("--build-arg=BUILDKIT_INLINE_CACHE=1"); // https://docs.docker.com/build/cache/backends/inline
        cmd.arg("--load");
    }

    if false {
        // TODO: https://docs.docker.com/build/attestations/
        cmd.arg("--provenance=mode=max");
        cmd.arg("--sbom=true");
    }
    //cmd.arg("--metadata-file=/tmp/meta.json"); => {"buildx.build.ref": "default/default/o5c4435yz6o6xxxhdvekx5lmn"}

    // TODO? pre-build rust stage with network, then never activate network ever.
    cmd.arg(format!("--network={}", runs_on_network()));

    cmd.arg("--platform=local");
    cmd.arg("--pull=false");
    cmd.arg(format!("--target={target}"));
    cmd.arg(format!("--output=type=local,dest={out_dir}"));
    // cmd.arg("--build-arg=BUILDKIT_MULTI_PLATFORM=1"); // "deterministic output"? adds /linux_amd64/ to extracted cratesio

    // TODO: do without local Docker-compatible CLI
    // https://github.com/pyaillet/doggy
    // https://lib.rs/crates/bollard

    for BuildContext { name, uri } in contexts {
        cmd.arg(format!("--build-context={name}={uri}"));
    }

    cmd.arg(format!("--file={dockerfile_path}"));

    cmd.arg(dockerfile_path.parent().unwrap_or(dockerfile_path));

    let call = cmd.show();
    let envs: Vec<_> = cmd.as_std().get_envs().map(|(k, v)| format!("{k:?}={v:?}")).collect();
    let envs = envs.join(" ");
    info!("Starting {call} (env: {envs:?})`");

    let errf = |e| anyhow!("Failed starting {call}: {e}");
    let start = Instant::now();
    let mut child = cmd.spawn().map_err(errf)?;

    // ---

    let pid = child.id().unwrap_or_default();
    info!("Started as pid={pid} in {:?}", start.elapsed());

    let out = TokioBufReader::new(child.stdout.take().expect("started")).lines();
    let err = TokioBufReader::new(child.stderr.take().expect("started")).lines();

    // TODO: try rawjson progress mode + find podman equivalent?
    let out_task = fwd(out, "stdout", "➤", MARK_STDOUT);
    let err_task = fwd(err, "stderr", "✖", MARK_STDERR);

    let (secs, code) = {
        let start = Instant::now();
        let res = child.wait().await;
        let elapsed = start.elapsed();
        (elapsed, res.map_err(|e| anyhow!("Failed calling {call}: {e}"))?.code())
    };
    info!("command `{command} build` ran in {secs:?}: {code:?}");

    let longish = Duration::from_secs(2);
    match join!(timeout(longish, out_task), timeout(longish, err_task)) {
        (Ok(Ok(())), Ok(Ok(()))) => {}
        (Ok(Err(e)), _) | (_, Ok(Err(e))) => {
            bail!("BUG: STDIO forwarding crashed: {e}")
        }
        (Err(Elapsed { .. }), _) | (_, Err(Elapsed { .. })) => {
            bail!("BUG: STDIO forwarding got crickets for {longish:?}")
        }
    }
    drop(child);

    if !(0..=1).contains(&code.unwrap_or(-1)) {
        // Something is very wrong here. Try to be helpful by logging some info about runner config:
        let mut cmd = Command::new(command);
        let cmd = cmd.kill_on_drop(true).arg("info");
        let Output { stdout, stderr, status } =
            cmd.output().await.map_err(|e| anyhow!("Failed starting {}: {e}", cmd.show()))?;
        let stdout = String::from_utf8_lossy(&stdout);
        let stderr = String::from_utf8_lossy(&stderr);
        warn!("Runner info: [code: {status}] [STDOUT {stdout}] [STDERR {stderr}]");
    }

    Ok(code)
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
) -> JoinHandle<()>
where
    R: AsyncBufRead + Unpin + Send + 'static,
{
    let fwder = if mark == MARK_STDOUT { fwd_stdout } else { fwd_stderr };
    spawn(async move {
        debug!("Starting {name} task {badge}");
        let start = Instant::now();
        let mut buf = String::new();
        let mut first = true;
        loop {
            let maybe_line = stdio.next_line().await;
            if first {
                first = false;
                debug!("Time To First Line for task {name}: {:?}", start.elapsed());
            }
            match maybe_line {
                Ok(None) => break,
                Ok(Some(line)) => {
                    if line.is_empty() {
                        continue;
                    }
                    debug!("{badge} {}", strip_ansi_escapes(&line));
                    if let Some(msg) = lift_stdio(&line, mark) {
                        fwder(msg, &mut buf);
                    }
                }
                Err(e) => {
                    warn!("Failed during piping of {name}: {e:?}");
                    break;
                }
            }
        }
        debug!("Terminating {name} task");
        drop(stdio);
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

    let msg = lift_stdio(lines[0], MARK_STDERR);
    assert_eq!(msg, Some(r#"{"$message_type":"artifact","artifact":"/tmp/thing","emit":"link""#));
    fwd_stderr(msg.unwrap(), &mut buf);
    assert_eq!(buf, r#"{"$message_type":"artifact","artifact":"/tmp/thing","emit":"link""#);

    let msg = lift_stdio(lines[1], MARK_STDERR);
    assert_eq!(msg, Some("}"));
    fwd_stderr(msg.unwrap(), &mut buf);
    assert_eq!(buf, "");

    // Then fwd_stderr
    // calls artifact_written(r#"{"$message_type":"artifact","artifact":"/tmp/thing","emit":"link"}"#)
    // which returns Some("/tmp/thing")
    use log::Level;
    assertx::assert_logs_contain_in_order!(logs, Level::Info => "rustc wrote /tmp/thing");
}

fn fwd_stderr(msg: &str, buf: &mut String) {
    let show = |msg: &str| {
        eprintln!("{msg}");
        if let Some(file) = artifact_written(msg) {
            // TODO: later assert said files were actually written, after runner completes
            info!("rustc wrote {file}") // FIXME: replace prefix target_path with '.'
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

#[inline]
fn fwd_stdout(msg: &str, #[expect(clippy::ptr_arg)] _buf: &mut String) {
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
