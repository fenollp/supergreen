//! Like `docker` / `moby` / `docker buildx` / `buildctl` 's connection helpers:
//! reach a [`RemoteDaemon`] via SSH, using a pool of connections.
//!
//! Using the system `ssh` keeps `~/.ssh/config` (aliases, `ProxyJump`, `IdentityFile`, ..) working.
//!
//! Channels are `ssh` processes running `docker system dial-stdio` (or `buildctl dial-stdio`)
//! remotely, multiplexed over a few `ControlMaster` connections ("lanes"): only these pay a handshake.
//! Lanes keep under sshd's `MaxSessions` (default: 10).
//!
//! ```text
//!   local                                                 ┊  remote host
//!                                                         ┊
//!   pool ─┬─ channel `ssh … dial-stdio` ─┐                ┊
//!         ├─ channel ────────────────────┼─ lane 0 ═══════╪═══ sshd ─┬─ dial-stdio ── daemon
//!         ├─ … (≤ 8 per lane) ───────────┘  ControlMaster ┊          ├─ dial-stdio ── daemon
//!         │                                 <name>-0.ctl  ┊          └─ … (one per channel)
//!         ├─ channel ────────────────────┬─ lane 1 ═══════╪═══ sshd ─── …
//!         ├─ … ──────────────────────────┘                ┊
//!         └─ … (≤ 8 lanes)                                ┊
//!
//!   ═══ one TCP connection, one SSH handshake
//!   ─── a session multiplexed on it: no handshake
//! ```
//!
//! A lane is created (and warmed with `ssh … true`) only once all others hold 8 channels.
//! Masters outlive us by `ControlPersist`, so the next `cargo green` run skips their handshake.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::{Context, Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info, warn};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::OnceCell,
};
use url::Host;

use crate::remote::{RemoteDaemon, pool::Link};

/// Max SSH master connections (each costs one handshake)
const MAX_LANES: usize = 8;

/// Max channels per master: under sshd's `MaxSessions` (OpenSSH default: 10)
pub(super) const MAX_PER_LANE: usize = 8;

/// Max live channels
pub(super) const MAX_CHANNELS: usize = MAX_LANES * MAX_PER_LANE;

fn ssh_exe() -> Result<PathBuf> {
    which::which("ssh").map_err(|e| anyhow!("No SSH client found: {e}"))
}

/// `ssh` using (or creating) the master connection at `control_path`
fn ssh(cmd: impl AsRef<Path>, control_path: &Utf8Path) -> Command {
    let mut cmd = Command::new(cmd.as_ref());
    // Not clearing env: a user's config can make ssh depend on any variable
    cmd.kill_on_drop(true);
    cmd.arg("-T"); // No pty: raw bytes
    cmd.arg("-o").arg("ControlMaster=auto");
    cmd.arg("-o").arg(format!("ControlPath={control_path}"));
    // Masters linger a bit for the next run, then go away (even if we crash)
    cmd.arg("-o").arg("ControlPersist=120");
    cmd.arg("-o").arg("ConnectTimeout=30");
    cmd.arg("-o").arg("ServerAliveInterval=30");
    cmd.arg("-o").arg("ServerAliveCountMax=3");
    cmd
}

impl RemoteDaemon {
    /// Remote command bridging a channel's stdio to the daemon
    pub(super) fn dial_stdio(&self) -> Vec<String> {
        let path = self.path();
        let (host, addr) = (format!("--host=unix://{path}"), format!("--addr=unix://{path}"));
        match (self, path.is_empty()) {
            (Self::Docker { .. }, true) => vec!["docker", "system", "dial-stdio"],
            (Self::Docker { .. }, false) => vec!["docker", &host, "system", "dial-stdio"],
            (Self::BuildKit { .. }, true) => vec!["buildctl", "dial-stdio"],
            (Self::BuildKit { .. }, false) => vec!["buildctl", &addr, "dial-stdio"],
        }
        .into_iter()
        .map(ToOwned::to_owned)
        .collect()
    }

    /// Args selecting the machine: after SSH options, before command
    pub(super) fn dest(&self) -> Vec<String> {
        let url = self.url();
        let mut args = vec![];
        if !url.username().is_empty() {
            args.extend(["-o".to_owned(), format!("User={}", url.username())]);
        }
        if let Some(port) = url.port() {
            args.extend(["-o".to_owned(), format!("Port={port}")]);
        }
        let host = match url.host() {
            Some(Host::Ipv6(ip)) => ip.to_string(), // Without brackets
            Some(host) => host.to_string(),
            None => unreachable!("PROOF: checked in RemoteDaemon::from_env()"),
        };
        args.extend(["--".to_owned(), host]);
        args
    }
}

/// One SSH master connection and its live-channel count.
struct Lane {
    id: usize,
    control_path: Utf8PathBuf,
    active: AtomicUsize,
    warmed: OnceCell<()>,
}

/// A channel on a lane.
pub(super) struct LaneSlot(Arc<Lane>);

impl Drop for LaneSlot {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Channels to the daemon, spread over master connections.
pub(super) struct Lanes {
    remote: RemoteDaemon,
    dir: Utf8PathBuf,
    name: String,
    lanes: Mutex<Vec<Arc<Lane>>>,
}

impl Lanes {
    pub(super) fn new(remote: &RemoteDaemon, dir: &Utf8Path, name: &str) -> Self {
        let (remote, dir, name) = (remote.to_owned(), dir.to_owned(), name.to_owned());
        Self { remote, dir, name, lanes: Mutex::new(Vec::new()) }
    }

    #[cfg(test)]
    pub(super) fn count(&self) -> usize {
        self.lanes.lock().unwrap().len()
    }

    /// Establishes a lane's master connection: one handshake.
    async fn warm(&self, Lane { id, control_path, .. }: &Lane) -> Result<()> {
        let mut cmd = ssh(ssh_exe()?, control_path);
        cmd.args(self.remote.dest()).arg("true");
        info!("Warming SSH conn: {cmd:?}");
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
        let out = cmd.output().await.map_err(|e| anyhow!("Failed spawning ssh: {e}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            bail!("Failed to ssh into {}: {stderr}", self.remote.url())
        }
        debug!("remote_pool: lane {id} up ({control_path})");
        Ok(())
    }

    /// Picks the least-loaded lane with room, creating one when all are full.
    /// Callers hold one of [`MAX_CHANNELS`] permits.
    fn choose_lane(&self) -> LaneSlot {
        let mut lanes = self.lanes.lock().unwrap();
        let lane = lanes
            .iter()
            .filter(|l| l.active.load(Ordering::Relaxed) < MAX_PER_LANE)
            .min_by_key(|l| l.active.load(Ordering::Relaxed))
            .cloned()
            .unwrap_or_else(|| {
                // Holding a permit means there is room for another lane
                let id = lanes.len();
                assert!(id < MAX_LANES, "BUG: more channels than permits");
                let control_path = self.dir.join(format!("{}-{id}.ctl", self.name));
                let lane = Arc::new(Lane {
                    id,
                    control_path,
                    active: AtomicUsize::new(0),
                    warmed: OnceCell::new(),
                });
                lanes.push(lane.clone());
                lane
            });
        lane.active.fetch_add(1, Ordering::Relaxed);
        drop(lanes); // Counted under the lock: no lane goes over MAX_PER_LANE
        LaneSlot(lane)
    }

    /// Opens a channel: cheap once its lane is warm (an SSH session over the existing master).
    pub(super) async fn open(&self) -> Result<Link> {
        let slot = self.choose_lane();
        let lane = &slot.0;
        lane.warmed.get_or_try_init(|| self.warm(lane)).await?;

        let mut cmd = ssh(ssh_exe()?, &lane.control_path);
        cmd.args(self.remote.dest());
        cmd.args(self.remote.dial_stdio());
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| anyhow!("Failed spawning ssh: {e}"))?;
        let stdin = child.stdin.take().context("remote_pool: no ssh stdin")?;
        let stdout = child.stdout.take().context("remote_pool: no ssh stdout")?;
        let stderr = child.stderr.take().context("remote_pool: no ssh stderr")?;
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // macOS: dial-stdio's half-closing a connection the daemon already closed gives ENOTCONN
                if line.contains("error while CloseRead") || line.contains("error while CloseWrite")
                {
                    debug!("remote_pool: remote: {line}");
                } else {
                    warn!("remote_pool: remote: {line}");
                    eprintln!("{line}");
                }
            }
        });
        // ssh forwards EOF on stdin, remote dial-stdio then does CloseWrite
        Ok(Link::new(stdout, stdin).with_child(child, slot))
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;

    use super::ssh;
    use crate::{redact::Vars, remote::RemoteDaemon};

    /// `ssh` destination args and the remote command, for an `ssh://` daemon
    fn remote<const N: usize>(env: [(&str, &str); N]) -> Option<(String, String)> {
        let remote = RemoteDaemon::from_env(&Vars::from(env)).unwrap()?;
        (remote.url().scheme() == "ssh")
            .then(|| (remote.dest().join(" "), remote.dial_stdio().join(" ")))
    }

    #[expect(clippy::unnecessary_wraps)]
    fn some(dest: &str, dial: &str) -> Option<(String, String)> {
        Some((dest.to_owned(), dial.to_owned()))
    }

    #[test]
    fn not_ssh() {
        assert_eq!(remote([]), None);
        assert_eq!(remote([(DOCKER_HOST!(), "unix:///var/run/docker.sock")]), None);
        assert_eq!(remote([(DOCKER_HOST!(), ""), (BUILDKIT_HOST!(), "tcp://thing:1234")]), None);
    }

    #[test]
    fn docker_host() {
        let got = remote([(DOCKER_HOST!(), "ssh://machine")]);
        assert_eq!(got, some("-- machine", "docker system dial-stdio"));
    }

    #[test]
    fn docker_host_with_user_and_port() {
        let got = remote([(DOCKER_HOST!(), "ssh://user@[::1]:22/")]);
        assert_eq!(got, some("-o User=user -o Port=22 -- ::1", "docker system dial-stdio"));
    }

    #[test]
    fn docker_host_with_socket() {
        // As docker does
        let got = remote([(DOCKER_HOST!(), "ssh://machine/run/user/1000/docker.sock")]);
        let dial = "docker --host=unix:///run/user/1000/docker.sock system dial-stdio";
        assert_eq!(got, some("-- machine", dial));
    }

    #[test]
    fn buildkit_host() {
        let got = remote([(BUILDKIT_HOST!(), "ssh://oomph")]);
        assert_eq!(got, some("-- oomph", "buildctl dial-stdio"));
        let got = remote([(BUILDKIT_HOST!(), "ssh://oomph/")]);
        assert_eq!(got, some("-- oomph", "buildctl dial-stdio"));
    }

    #[test]
    fn buildkit_host_with_socket() {
        let got = remote([(BUILDKIT_HOST!(), "ssh://me@h:2222/run/buildkit/buildkitd.sock")]);
        assert_eq!(
            got,
            some(
                "-o User=me -o Port=2222 -- h",
                "buildctl --addr=unix:///run/buildkit/buildkitd.sock dial-stdio"
            )
        );
    }

    #[test]
    fn bad_hosts() {
        fn bad<const N: usize>(env: [(&str, &str); N]) -> bool {
            RemoteDaemon::from_env(&Vars::from(env)).is_err()
        }
        assert!(bad([(DOCKER_HOST!(), "ssh://big"), (BUILDKIT_HOST!(), "ssh://big")]));
        assert!(bad([(BUILDKIT_HOST!(), "ssh://")]));
        assert!(bad([(BUILDKIT_HOST!(), "ssh://host:port")]));
    }

    #[test]
    fn ssh_options() {
        let cmd = ssh("ssh", Utf8Path::new("/run/pool-0.ctl"));
        let args: Vec<_> = cmd.as_std().get_args().map(|arg| arg.to_str().unwrap()).collect();
        assert_eq!(cmd.as_std().get_program(), "ssh");
        assert!(args.contains(&"ControlPath=/run/pool-0.ctl"), "{args:?}");
        assert!(args.contains(&"ControlMaster=auto"), "{args:?}");
    }
}
