//! Warm, multiplexed SSH connections for remote (`DOCKER_HOST=ssh://…`) builds.
//!
//! cargo-green runs **one BuildKit build per `rustc` invocation** by shelling out
//! to `docker buildx`. With `DOCKER_HOST=ssh://extra-oomph` each of those calls
//! launches its own `ssh … docker system dial-stdio` and pays a full SSH
//! handshake (TCP + key exchange + auth). Across a workspace that is thousands of
//! handshakes — pure latency.
//!
//! The **long-lived `cargo green` parent** starts a [`DockerPool`]: a local unix
//! socket backed by a small pool of `ssh -o ControlMaster` connections, each
//! channel running `docker system dial-stdio` on the remote:
//!
//! ```text
//!   cargo green (main, owns the pool)
//!     ├── unix:///run/user/1000/supergreen-docker-<target-hash>.sock
//!     └── spawns: cargo
//!            └── spawns: cargo-green (RUSTC_WRAPPER, per crate)
//!                   └── spawns: docker buildx build   --DOCKER_HOST=unix://…sock-->
//! ```
//!
//! Design notes:
//!
//! * The remote command is `docker system dial-stdio`: a transparent pipe to the
//!   remote **dockerd Engine API** socket — the exact protocol `docker` expects
//!   behind a `DOCKER_HOST`, and what its own ssh:// connhelper runs. REST,
//!   hijacked streams and buildx's gRPC-over-Engine-API all pass through
//!   unchanged. Its cousin `docker buildx dial-stdio` instead exposes a builder's
//!   BuildKit **gRPC** API: only suitable as a `BUILDKIT_HOST`/`remote`-driver
//!   endpoint, never as a `DOCKER_HOST`.
//! * The socket path is a stable function of the ssh target, not of the run:
//!   `buildx create` records the then-current `DOCKER_HOST` as the managed
//!   builder's node endpoint, and that recorded endpoint must resolve again on
//!   the next run. A later `cargo green` rebinds the socket, or joins a live one
//!   as [`DockerPool::Shared`].
//! * Wrapper subprocesses learn the path through the `Green` config they
//!   deserialize (`Green::docker_pool_sock`); `Green::cmd()` points just the
//!   spawned `docker` at it. cargo-green's own remote-detection keeps reading
//!   the real ssh:// value from `runner_envs`, unaffected.
//! * The transport drives the **system `ssh` binary** with `ControlMaster=auto`
//!   and `ControlPersist`, so the handshake happens once and every later dial is
//!   a cheap multiplexed channel. Real `ssh` keeps `~/.ssh/config` working —
//!   `Host` aliases like `extra-oomph`, `ProxyJump`, `IdentityFile`.
//! * A small **lane pool** spreads concurrent channels across several SSH master
//!   connections so we don't exceed the server's `MaxSessions` (default 10):
//!   N parallel builds cost a handful of handshakes total, not N.

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::{Context, Result, anyhow, bail};
use log::{debug, info, warn};
use tokio::{
    io::AsyncWriteExt,
    net::{UnixListener, UnixStream},
    process::{Child, ChildStdin, ChildStdout, Command as TokioCommand},
    sync::{OnceCell, OwnedSemaphorePermit, Semaphore, watch},
    task::JoinHandle,
};

use crate::{green::Green, runner::Runner};

/// Pool sockets are named `{POOL_SOCK_PREFIX}{target-hash}.sock`. builder.rs
/// also uses this marker to spot (possibly dead) pool endpoints recorded in
/// buildx's state from earlier remoting runs.
pub(crate) const POOL_SOCK_PREFIX: &str = "supergreen-docker-";

/// The pool, if one is warranted: [`Runner::Docker`] with an ssh:// `DOCKER_HOST`.
pub(crate) enum DockerPool {
    /// This process owns the socket and the SSH masters behind it.
    Owned(PoolProxy),
    /// Another live `cargo green` (same ssh target) already serves this socket:
    /// piggyback on it. If its owner exits first, later dials fail and `docker`
    /// calls surface that error.
    Shared(PathBuf),
}

impl DockerPool {
    /// Start (or join) a pool when `green` builds through a remote ssh:// docker
    /// daemon. `Ok(None)` otherwise: local/tcp daemons don't need pooling.
    pub(crate) async fn maybe_start(green: &Green) -> Result<Option<Self>> {
        if green.runner != Runner::Docker {
            return Ok(None);
        }
        let Some(host) = green.runner_envs.get(DOCKER_HOST!()) else { return Ok(None) };
        if !host.starts_with("ssh://") {
            return Ok(None);
        }
        let target = parse_ssh_url(host)
            .ok_or_else(|| anyhow!("dockerpool: unusable ssh {}={host:?}", DOCKER_HOST!()))?;

        let socket_path = socket_path_for(&target);
        if UnixStream::connect(&socket_path).await.is_ok() {
            info!("dockerpool: joining live pool socket unix://{}", socket_path.display());
            return Ok(Some(Self::Shared(socket_path)));
        }
        let _ = std::fs::remove_file(&socket_path); // Stale leftover, if any

        let proxy = PoolProxy::start(target, socket_path, PoolOpts::default()).await?;
        Ok(Some(Self::Owned(proxy)))
    }

    pub(crate) fn socket_path(&self) -> &Path {
        match self {
            Self::Owned(proxy) => proxy.socket_path(),
            Self::Shared(socket_path) => socket_path,
        }
    }

    /// Stop accepting, tear down the SSH masters, and remove the socket.
    /// No-op for a [`DockerPool::Shared`] socket: its owner cleans up.
    pub(crate) async fn shutdown(self) {
        match self {
            Self::Owned(proxy) => proxy.shutdown().await,
            Self::Shared(..) => {}
        }
    }
}

// ----------------------------------------------------------------------------
// SSH target + options
// ----------------------------------------------------------------------------

/// A parsed `ssh://[user@]host[:port]` destination.
#[derive(Clone, Debug)]
pub(crate) struct SshTarget {
    pub(crate) user: Option<String>,
    pub(crate) host: String,
    pub(crate) port: Option<String>,
}

impl SshTarget {
    fn key(&self) -> String {
        format!(
            "{}@{}:{}",
            self.user.as_deref().unwrap_or(""),
            self.host,
            self.port.as_deref().unwrap_or("")
        )
    }
}

/// Parse a `DOCKER_HOST` ssh URL. Returns `None` if it isn't a usable ssh URL.
pub(crate) fn parse_ssh_url(s: &str) -> Option<SshTarget> {
    let rest = s.strip_prefix("ssh://")?;
    // Authority is everything before any path/query.
    let authority = rest.split(['/', '?']).next().unwrap_or(rest);
    let (user, hostport) = match authority.rsplit_once('@') {
        Some((u, hp)) => (Some(u.to_owned()), hp),
        None => (None, authority),
    };
    let (host, port) = if let Some(stripped) = hostport.strip_prefix('[') {
        // IPv6 literal: [::1]:22
        let (h, after) = stripped.split_once(']')?;
        (h.to_owned(), after.strip_prefix(':').map(ToOwned::to_owned))
    } else if let Some((h, p)) = hostport.rsplit_once(':') {
        (h.to_owned(), Some(p.to_owned()))
    } else {
        (hostport.to_owned(), None)
    };
    if host.is_empty() {
        return None;
    }
    Some(SshTarget { user, host, port })
}

/// Tunables for the pool. `Default` is sensible for a developer laptop.
#[derive(Clone, Debug)]
pub(crate) struct PoolOpts {
    /// ssh binary name/path. Default `"ssh"`.
    pub(crate) ssh_bin: String,
    /// Command run on the remote side of each channel. Must speak the Engine API
    /// (see module docs), hence `["docker", "system", "dial-stdio"]`.
    pub(crate) remote_cmd: Vec<String>,
    /// Max number of SSH master connections (each its own handshake). Default 4.
    pub(crate) max_lanes: usize,
    /// Max concurrent channels per master, kept under the server's `MaxSessions`
    /// (OpenSSH default 10). Default 8.
    pub(crate) max_per_lane: usize,
    /// ssh `ControlPersist` (seconds, or a duration like `"10m"`). Default `"600"`.
    pub(crate) control_persist: String,
    /// ssh `ConnectTimeout` in seconds. Default `"20"`.
    pub(crate) connect_timeout: String,
    /// Extra ssh args, e.g. `["-o", "BatchMode=yes"]`.
    pub(crate) extra_ssh_args: Vec<String>,
}

impl Default for PoolOpts {
    fn default() -> Self {
        Self {
            ssh_bin: "ssh".into(),
            remote_cmd: vec!["docker".into(), "system".into(), "dial-stdio".into()],
            max_lanes: 4,
            max_per_lane: 8,
            control_persist: "600".into(),
            connect_timeout: "20".into(),
            extra_ssh_args: Vec::new(),
        }
    }
}

// ----------------------------------------------------------------------------
// Lane + Pool
// ----------------------------------------------------------------------------

/// One SSH master connection and its live-channel count.
struct Lane {
    id: usize,
    control_path: PathBuf,
    active: AtomicUsize,
    warmed: OnceCell<()>,
}

impl Lane {
    fn new(id: usize, control_path: PathBuf) -> Self {
        Self { id, control_path, active: AtomicUsize::new(0), warmed: OnceCell::new() }
    }
}

/// The pool of warm SSH master connections. Owned by the main process.
struct Pool {
    target: SshTarget,
    opts: PoolOpts,
    control_dir: PathBuf,
    /// A permit is held for the *entire lifetime* of a channel, which bounds
    /// total live channels to `max_lanes * max_per_lane` and lets [`Pool::dial`]
    /// guarantee a free lane slot exists (pigeonhole) before assigning one.
    sem: Arc<Semaphore>,
    lanes: Mutex<Vec<Arc<Lane>>>,
}

impl Pool {
    fn new(target: SshTarget, opts: PoolOpts) -> Self {
        let control_dir = std::env::temp_dir().join("supergreen-ssh");
        if let Err(e) = std::fs::create_dir_all(&control_dir) {
            warn!("dockerpool: cannot create {}: {e}", control_dir.display());
        }
        let permits = opts.max_lanes.max(1) * opts.max_per_lane.max(1);
        Self {
            target,
            control_dir,
            sem: Arc::new(Semaphore::new(permits)),
            lanes: Mutex::new(Vec::new()),
            opts,
        }
    }

    fn control_path_for(&self, id: usize) -> PathBuf {
        let mut h = DefaultHasher::new();
        self.target.key().hash(&mut h);
        // Short name: unix socket paths are length-capped (~104 bytes on macOS).
        self.control_dir.join(format!("cm-{:016x}-{id}", h.finish()))
    }

    /// ssh options that select/share a given lane's master connection. They must
    /// accompany both real dials and `-O` control commands.
    fn ctrl_args(&self, lane: &Lane) -> Vec<String> {
        let mut a = vec![
            "-o".into(),
            "ControlMaster=auto".into(),
            "-o".into(),
            format!("ControlPath={}", lane.control_path.display()),
            "-o".into(),
            format!("ControlPersist={}", self.opts.control_persist),
            "-o".into(),
            format!("ConnectTimeout={}", self.opts.connect_timeout),
            "-o".into(),
            "ServerAliveInterval=30".into(),
            "-o".into(),
            "ServerAliveCountMax=3".into(),
        ];
        if let Some(u) = &self.target.user {
            a.push("-l".into());
            a.push(u.clone());
        }
        if let Some(p) = &self.target.port {
            a.push("-p".into());
            a.push(p.clone());
        }
        a.extend(self.opts.extra_ssh_args.iter().cloned());
        a
    }

    /// Establish a lane's master connection (one handshake). Idempotent across
    /// concurrent callers via the lane's `OnceCell`.
    async fn warm_lane(&self, lane: &Lane) -> Result<()> {
        let mut args = self.ctrl_args(lane);
        args.push(self.target.host.clone());
        args.push("true".into()); // trivial remote command just forces master setup
        let status = TokioCommand::new(&self.opts.ssh_bin)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .status()
            .await?;
        if !status.success() {
            bail!(
                "dockerpool: ssh warm-up to {} failed (exit {:?})",
                self.target.host,
                status.code()
            );
        }
        debug!("dockerpool: lane {} warm ({})", lane.id, lane.control_path.display());
        Ok(())
    }

    /// Warm the first lane so the very first build is fast.
    async fn prewarm(&self) -> Result<()> {
        let lane = {
            let mut lanes = self.lanes.lock().unwrap();
            if let Some(l) = lanes.first() {
                l.clone()
            } else {
                let l = Arc::new(Lane::new(0, self.control_path_for(0)));
                lanes.push(l.clone());
                l
            }
        };
        lane.warmed.get_or_try_init(|| self.warm_lane(&lane)).await.map(|_| ())
    }

    /// Pick the least-loaded lane with a free channel slot, creating a new lane if
    /// every existing one is full. Increments the chosen lane's active count;
    /// the returned channel's [`Upstream`] drop decrements it.
    fn choose_lane(&self) -> Arc<Lane> {
        let mut lanes = self.lanes.lock().unwrap();
        let mut best: Option<Arc<Lane>> = None;
        for l in lanes.iter() {
            if l.active.load(Ordering::Relaxed) >= self.opts.max_per_lane {
                continue;
            }
            match &best {
                Some(b) if b.active.load(Ordering::Relaxed) <= l.active.load(Ordering::Relaxed) => {
                }
                _ => best = Some(l.clone()),
            }
        }
        let lane = best.unwrap_or_else(|| {
            // Safe because we hold a semaphore permit: total live channels are
            // bounded, so we can only get here with room for another lane.
            debug_assert!(lanes.len() < self.opts.max_lanes);
            let id = lanes.len();
            let l = Arc::new(Lane::new(id, self.control_path_for(id)));
            lanes.push(l.clone());
            l
        });
        lane.active.fetch_add(1, Ordering::Relaxed);
        lane
    }

    fn spawn_dial_stdio(&self, lane: &Lane) -> Result<(Child, ChildStdin, ChildStdout)> {
        let mut args = self.ctrl_args(lane);
        args.push(self.target.host.clone());
        args.extend(self.opts.remote_cmd.iter().cloned());
        let mut child = TokioCommand::new(&self.opts.ssh_bin)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let stdin = child.stdin.take().context("dockerpool: no ssh stdin")?;
        let stdout = child.stdout.take().context("dockerpool: no ssh stdout")?;
        Ok((child, stdin, stdout))
    }

    /// Open a fresh, multiplexed channel to the remote daemon. Cheap once a lane
    /// is warm: just an SSH session over the existing master.
    async fn dial(&self) -> Result<Upstream> {
        let permit = self.sem.clone().acquire_owned().await.context("dockerpool: pool closed")?;
        let lane = self.choose_lane();

        if let Err(e) = lane.warmed.get_or_try_init(|| self.warm_lane(&lane)).await {
            lane.active.fetch_sub(1, Ordering::Relaxed);
            return Err(e);
        }
        match self.spawn_dial_stdio(&lane) {
            Ok((child, stdin, stdout)) => Ok(Upstream {
                child,
                stdin: Some(stdin),
                stdout: Some(stdout),
                _permit: permit,
                lane,
            }),
            Err(e) => {
                lane.active.fetch_sub(1, Ordering::Relaxed);
                Err(e)
            }
        }
    }

    /// Tear down every master connection (`ssh -O exit`) and remove control sockets.
    async fn shutdown(&self) {
        let lanes = { self.lanes.lock().unwrap().clone() };
        for lane in lanes {
            let mut args = self.ctrl_args(&lane);
            args.push("-O".into());
            args.push("exit".into());
            args.push(self.target.host.clone());
            let _ = TokioCommand::new(&self.opts.ssh_bin)
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
            let _ = std::fs::remove_file(&lane.control_path);
        }
    }
}

/// A live channel to the remote daemon. Holds its lane slot + semaphore permit
/// for its whole lifetime; dropping it frees both and (via `kill_on_drop`) reaps
/// the ssh process if it hasn't exited.
struct Upstream {
    child: Child,
    /// Taken by [`handle`]; dropping it closes the fd = EOF for the remote.
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    _permit: OwnedSemaphorePermit,
    lane: Arc<Lane>,
}

impl Upstream {
    /// Wait for the (already half-closed) ssh process to exit cleanly.
    async fn finish(mut self) {
        let _ = self.child.wait().await;
        // active count is decremented by Drop.
    }
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.lane.active.fetch_sub(1, Ordering::Relaxed);
    }
}

// ----------------------------------------------------------------------------
// Proxy
// ----------------------------------------------------------------------------

/// The unix-socket proxy. Accepts local connections and bridges each to a warm
/// pooled channel. Start it through [`DockerPool::maybe_start`].
pub(crate) struct PoolProxy {
    socket_path: PathBuf,
    pool: Arc<Pool>,
    accept_stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl PoolProxy {
    /// Bind the socket, warm the first lane, and start accepting.
    async fn start(target: SshTarget, socket_path: PathBuf, opts: PoolOpts) -> Result<Self> {
        let listener = UnixListener::bind(&socket_path)?;
        // Owner-only: this socket is a direct line to the (remote) daemon.
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;

        let pool = Arc::new(Pool::new(target, opts));
        pool.prewarm().await?; // pay the one handshake now

        let (accept_stop, mut stop_rx) = watch::channel(false);
        let pool2 = pool.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop_rx.changed() => break,
                    accepted = listener.accept() => match accepted {
                        Ok((conn, _)) => {
                            let p = pool2.clone();
                            tokio::spawn(async move { handle(p, conn).await });
                        }
                        Err(e) => { warn!("dockerpool: accept failed: {e}"); break; }
                    },
                }
            }
        });

        info!("dockerpool: listening on unix://{}", socket_path.display());
        Ok(Self { socket_path, pool, accept_stop, task })
    }

    fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Stop accepting, tear down the SSH masters, and remove the socket.
    async fn shutdown(mut self) {
        let _ = self.accept_stop.send(true);
        let _ = (&mut self.task).await;
        self.pool.shutdown().await;
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

impl Drop for PoolProxy {
    fn drop(&mut self) {
        // Best-effort for panic/early-exit paths ([`PoolProxy::shutdown`] is the
        // graceful route; a second remove is harmless). The ssh masters aren't
        // reachable from a sync Drop: they self-expire through `ControlPersist`.
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Bridge one accepted local connection to a pooled upstream channel.
///
/// Copies each direction independently (not `copy_bidirectional`: tokio's
/// `ChildStdin::poll_shutdown` doesn't close the fd) so that EOF propagates as a
/// half-close both ways — the `CloseWrite` dance moby's commandconn does. The
/// draining side keeps flowing after the other side finishes.
async fn handle(pool: Arc<Pool>, mut local: UnixStream) {
    let mut up = match pool.dial().await {
        Ok(u) => u,
        Err(e) => {
            warn!("dockerpool: upstream dial failed: {e}");
            return; // dropping `local` closes it
        }
    };
    let mut to_remote = up.stdin.take().expect("set in dial()");
    let mut from_remote = up.stdout.take().expect("set in dial()");
    let (mut local_read, mut local_write) = local.split();

    let up_dir = async {
        let n = tokio::io::copy(&mut local_read, &mut to_remote).await;
        drop(to_remote); // close ssh's stdin: the remote reads EOF
        n
    };
    let down_dir = async {
        let n = tokio::io::copy(&mut from_remote, &mut local_write).await;
        let _ = local_write.shutdown().await; // half-close towards the local client
        n
    };
    match tokio::join!(up_dir, down_dir) {
        (Ok(to_remote), Ok(to_local)) => {
            debug!("dockerpool: session done ({to_remote} up / {to_local} down bytes)");
        }
        (up_res, down_res) => {
            debug!("dockerpool: session ended: up:{up_res:?} down:{down_res:?}");
        }
    }
    up.finish().await;
}

/// Stable per-target socket path: the managed builder records it as its node
/// endpoint at `buildx create` time and must find it live on later runs too.
fn socket_path_for(target: &SshTarget) -> PathBuf {
    let mut h = DefaultHasher::new();
    target.key().hash(&mut h);
    // XDG_RUNTIME_DIR (e.g. /run/user/1000) is short and tmpfs-backed on Linux.
    // Falls back to the temp dir elsewhere. NOTE on macOS: the temp dir path can
    // be long and unix sockets are capped (~104 bytes); set a short $TMPDIR if so.
    let dir =
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join(format!("{POOL_SOCK_PREFIX}{:016x}.sock", h.finish()))
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    #[test]
    fn parses_ssh_urls() {
        let t = parse_ssh_url("ssh://extra-oomph").unwrap();
        assert_eq!(t.host, "extra-oomph");
        assert!(t.user.is_none() && t.port.is_none());

        let t = parse_ssh_url("ssh://me@beaffy-machine.internal.net:2222").unwrap();
        assert_eq!(t.user.as_deref(), Some("me"));
        assert_eq!(t.host, "beaffy-machine.internal.net");
        assert_eq!(t.port.as_deref(), Some("2222"));

        let t = parse_ssh_url("ssh://user@[::1]:22").unwrap();
        assert_eq!(t.host, "::1");
        assert_eq!(t.port.as_deref(), Some("22"));

        assert!(parse_ssh_url("tcp://localhost:2375").is_none());
        assert!(parse_ssh_url("ssh://").is_none());
    }

    #[test]
    fn socket_path_is_stable_per_target() {
        let a = SshTarget { user: None, host: "gol".into(), port: None };
        let b = SshTarget { user: Some("me".into()), host: "gol".into(), port: None };
        assert_eq!(socket_path_for(&a), socket_path_for(&a));
        assert_ne!(socket_path_for(&a), socket_path_for(&b));
        let name = socket_path_for(&a).file_name().unwrap().to_str().unwrap().to_owned();
        assert!(name.starts_with(POOL_SOCK_PREFIX), "{name}");
    }

    /// End to end through a fake `ssh` that pipes stdio like dial-stdio would:
    /// bytes round-trip, and (crucially) the client's half-close reaches the
    /// remote command as EOF — otherwise `cat` would never terminate.
    #[tokio::test]
    async fn pool_proxy_pipes_and_half_closes() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("supergreen-pooltest-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let fake_ssh = dir.join("fake-ssh");
        std::fs::write(&fake_ssh, "#!/bin/sh\nexec cat\n")?;
        std::fs::set_permissions(&fake_ssh, std::fs::Permissions::from_mode(0o755))?;

        let target = SshTarget { user: None, host: "pooltest".into(), port: None };
        let opts = PoolOpts { ssh_bin: fake_ssh.display().to_string(), ..PoolOpts::default() };
        let socket_path = dir.join("pool.sock");
        let _ = std::fs::remove_file(&socket_path);
        let proxy = PoolProxy::start(target, socket_path.clone(), opts).await?;

        let mut conn = UnixStream::connect(&socket_path).await?;
        conn.write_all(b"ping through the pool").await?;
        conn.shutdown().await?; // half-close: cat sees EOF, echoes, exits
        let mut echoed = Vec::new();
        conn.read_to_end(&mut echoed).await?;
        assert_eq!(echoed, b"ping through the pool");

        proxy.shutdown().await;
        assert!(!socket_path.exists());
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
