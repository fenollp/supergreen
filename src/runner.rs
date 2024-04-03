use std::{
    collections::BTreeMap,
    process::Stdio,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use camino::Utf8Path;
use tokio::{
    io::{AsyncBufReadExt, BufReader as TokioBufReader},
    join,
    process::Command,
    spawn,
    time::timeout,
};

pub(crate) const MARK_STDOUT: &str = "::STDOUT:: ";
pub(crate) const MARK_STDERR: &str = "::STDERR:: ";

pub(crate) async fn build(
    krate: &str,
    command: &str,
    dockerfile_path: impl AsRef<Utf8Path>,
    target: &str,
    contexts: &BTreeMap<String, String>,
    out_dir: impl AsRef<Utf8Path>,
) -> Result<Option<i32>> {
    let mut cmd = Command::new(command);
    cmd.arg("--debug");
    cmd.arg("build");

    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());

    // Makes sure the underlying OS process dies with us
    cmd.kill_on_drop(true);

    // Makes sure that the BuildKit builder is used by either runner
    cmd.env("DOCKER_BUILDKIT", "1");

    if false {
        cmd.arg("--no-cache");
    }
    cmd.arg("--network=none");
    cmd.arg("--platform=local");
    cmd.arg("--pull=false");
    cmd.arg(format!("--target={target}"));
    cmd.arg(format!("--output=type=local,dest={out_dir}", out_dir = out_dir.as_ref()));
    cmd.arg(format!("--file={dockerfile_path}", dockerfile_path = dockerfile_path.as_ref()));

    for (name, uri) in contexts {
        cmd.arg(format!("--build-context={name}={uri}"));
    }

    cmd.arg(dockerfile_path.as_ref().parent().unwrap_or(dockerfile_path.as_ref()));
    let args = cmd.as_std().get_args().map(|x| x.to_string_lossy().to_string()).collect::<Vec<_>>();
    let args = args.join(" ");

    log::info!(target:&krate, "Starting `{command} {args}`");
    let errf = || format!("Failed starting `{command} {args}`");
    let mut child = cmd.spawn().with_context(errf)?;

    let pid = child.id().unwrap_or_default();
    log::info!(target:&krate, "Started `{command} {args}` as pid={pid}`");

    let krate_clone = krate.to_owned();
    let mut out = TokioBufReader::new(child.stdout.take().expect("started")).lines();
    let stdout = spawn(async move {
        log::debug!(target:&krate_clone, "Starting STDOUT-{pid} task");
        loop {
            match out.next_line().await {
                Ok(None) => break,
                Ok(Some(line)) => {
                    log::debug!(target:&krate_clone, "➤ {line}");
                    if let Some(msg) = lift_stdio(&line, MARK_STDOUT) {
                        println!("{msg}");
                    }
                }
                Err(e) => {
                    log::warn!("Failed during piping of STDOUT-{pid}: {e:?}");
                    break;
                }
            }
        }
        log::debug!(target:&krate_clone, "Going down: STDOUT-{pid} task");
        drop(out);
    });

    let krate_clone = krate.to_owned();
    let mut err = TokioBufReader::new(child.stderr.take().expect("started")).lines();
    let stderr = spawn(async move {
        log::debug!(target:&krate_clone, "Starting STDERR-{pid} task");
        loop {
            match err.next_line().await {
                Ok(None) => break,
                Ok(Some(line)) => {
                    log::debug!(target:&krate_clone, "✖ {line}");
                    if let Some(msg) = lift_stdio(&line, MARK_STDERR) {
                        eprintln!("{msg}");
                        if let Some(file) = artifact_written(msg) {
                            log::info!(target:&krate_clone, "rustc wrote {file}")
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Failed during piping of STDERR-{pid}: {e:?}");
                    break;
                }
            }
        }
        log::debug!(target:&krate_clone, "Going down: STDERR-{pid} task");
        drop(err);
    });

    let (secs, code) = {
        let start = Instant::now();
        let res = child.wait().await;
        let elapsed = start.elapsed();
        (elapsed, res.with_context(|| format!("Failed calling `{command} {args}`"))?.code())
    };
    log::info!("command `{command} build` ran in {secs:?}: {code:?}");
    let longish = Duration::from_secs(2);
    let (_, _) = join!(timeout(longish, stdout), timeout(longish, stderr));
    drop(child);

    if code == Some(255) {
        let check = Command::new(command)
            .kill_on_drop(true)
            .arg("info")
            .output()
            .await
            .with_context(|| format!("Failed starting `{command} info`"))?;
        let stdout = String::from_utf8(check.stdout).context("Failed parsing check STDOUT")?;
        let stderr = String::from_utf8(check.stderr).context("Failed parsing check STDERR")?;
        log::warn!(target:&krate, "Runner info: [code: {}] [STDOUT {}] [STDERR {}]", check.status, stdout, stderr);
    }

    Ok(code)
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
#[inline]
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

#[inline]
fn lift_stdio<'a>(line: &'a str, mark: &'static str) -> Option<&'a str> {
    // Docker builds running shell code usually start like: #47 0.057
    let line = line.trim_start_matches(|c| ['#', '.', ' '].contains(&c) || c.is_ascii_digit());
    let msg = line.trim_start_matches(mark);
    let cut = msg.len() != line.len();
    let msg = msg.trim();
    (cut && !msg.is_empty()).then_some(msg)
}
