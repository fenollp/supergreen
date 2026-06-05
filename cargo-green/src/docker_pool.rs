//! Warm, multiplexed Docker/BuildKit connections for cargo-green's remote builds.
//!
//! cargo-green runs **one BuildKit build per `rustc` invocation** by shelling out
//! to `docker buildx`. With `DOCKER_HOST=ssh://extra-oomph` each of those calls
//! launches its own `ssh … docker system dial-stdio` and pays a full SSH
//! handshake (TCP + key exchange + auth). Across a workspace that is thousands of
//! handshakes — pure latency.
//!
//! This module removes that cost. The **long-lived `cargo green` process** owns a
//! pool of warm SSH connections and exposes them on a local **unix socket**. The
//! short-lived `rustc`-wrapper subprocesses (and the `docker buildx` they spawn)
//! connect to that socket instead of opening their own SSH connection:
//!
//! ```text
//!   cargo green (main, owns the pool)
//!     ├── unix:///run/user/1000/supergreen-docker-<pid>.sock   <-- the pool
//!     └── spawns: cargo
//!            └── spawns: cargo-green (RUSTC_WRAPPER, per crate)
//!                   └── spawns: docker buildx build   --DOCKER_HOST=unix://…sock-->
//! ```
//!
//! Design notes, mirroring moby's `docker system dial-stdio`
//! (cli/command/system/dial_stdio.go):
//!
//! * The transport drives the **system `ssh` binary** with `ControlMaster=auto`
//!   and `ControlPersist`, so the handshake happens once and every later dial is a
//!   cheap multiplexed channel running `docker system dial-stdio` on the remote.
//!   Using real `ssh` keeps `~/.ssh/config` working — `Host` aliases like
//!   `extra-oomph`, `ProxyJump`, `IdentityFile` — which supergreen relies on.
//! * The proxy is a **transparent byte pipe** to the remote daemon socket, so all
//!   Docker traffic works unchanged: the HTTP/1.1 REST API and, crucially, the
//!   HTTP/2 **gRPC** that buildx uses to talk to BuildKit. gRPC stream
//!   multiplexing is preserved end to end between the local buildx process and
//!   remote buildkitd; the proxy never parses or reframes it.
//! * A small **lane pool** spreads concurrent channels across several SSH master
//!   connections so we don't exceed the server's `MaxSessions` (default 10):
//!   N parallel builds cost a handful of handshakes total, not N.
//!
//! Two entry points are provided:
//!   * [`PoolProxy`] — the async API, used by cargo-green's `#[tokio::main]` parent.
//!   * [`ProxyHandle`] — a fully synchronous wrapper around it, for a sync `main`.

// Fuller API surface (sync handle, accessors) than the parent currently wires up.
#![allow(dead_code)]

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
    io::Join,
    net::{UnixListener, UnixStream},
    process::{Child, ChildStdin, ChildStdout, Command as TokioCommand},
    sync::{OnceCell, OwnedSemaphorePermit, Semaphore, watch},
    task::JoinHandle,
};

/// Env var the main process sets so wrapper subprocesses know where the pool is.
/// The runner reads it via [`pooled_docker_host`] and points `docker` at it.
pub const POOL_SOCK_ENV: &str = "CARGOGREEN_DOCKER_POOL_SOCK";

/// For the subprocess side: if a pool socket was published by the main process,
/// return the `DOCKER_HOST` value to set on the spawned `docker`/`podman` command.
///
/// ```ignore
/// let mut docker = std::process::Command::new("docker");
/// // …build args…
/// if let Some(host) = crate::docker_pool::pooled_docker_host() {
///     docker.env("DOCKER_HOST", host); // redirect just this child to the pool
/// }
/// ```
///
/// Note: this only overrides `DOCKER_HOST` for the `docker` child. cargo-green's
/// own "am I building remotely?" logic keeps reading the real `ssh://` value, so
/// remote/local detection is unaffected.
pub fn pooled_docker_host() -> Option<String> {
    std::env::var(POOL_SOCK_ENV).ok().map(|s| format!("unix://{s}"))
}

// ----------------------------------------------------------------------------
// SSH target + options
// ----------------------------------------------------------------------------

/// A parsed `ssh://[user@]host[:port]` destination.
#[derive(Clone, Debug)]
pub struct SshTarget {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<String>,
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
pub fn parse_ssh_url(s: &str) -> Option<SshTarget> {
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
pub struct PoolOpts {
    /// ssh binary name/path. Default `"ssh"`.
    pub ssh_bin: String,
    /// Command run on the remote side of each channel.
    /// Default `["docker", "system", "dial-stdio"]`. Use
    /// `["docker", "buildx", "dial-stdio"]` to land on the BuildKit gRPC API.
    pub remote_cmd: Vec<String>,
    /// Max number of SSH master connections (each its own handshake). Default 4.
    pub max_lanes: usize,
    /// Max concurrent channels per master, kept under the server's `MaxSessions`
    /// (OpenSSH default 10). Default 8.
    pub max_per_lane: usize,
    /// ssh `ControlPersist` (seconds, or a duration like `"10m"`). Default `"600"`.
    pub control_persist: String,
    /// ssh `ConnectTimeout` in seconds. Default `"20"`.
    pub connect_timeout: String,
    /// Extra ssh args, e.g. `["-o", "BatchMode=yes"]`.
    pub extra_ssh_args: Vec<String>,
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
    #[allow(dead_code)]
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
pub struct Pool {
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
    pub fn new(target: SshTarget, opts: PoolOpts) -> Self {
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
    pub async fn prewarm(&self) -> Result<()> {
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

    fn spawn_dial_stdio(&self, lane: &Lane) -> Result<(Child, Join<ChildStdout, ChildStdin>)> {
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
        // join() presents (stdout-as-reader, stdin-as-writer) as one duplex conn.
        Ok((child, tokio::io::join(stdout, stdin)))
    }

    /// Open a fresh, multiplexed channel to the remote daemon. Cheap once a lane
    /// is warm: just an SSH session over the existing master.
    pub async fn dial(&self) -> Result<Upstream> {
        let permit = self.sem.clone().acquire_owned().await.context("dockerpool: pool closed")?;
        let lane = self.choose_lane();

        if let Err(e) = lane.warmed.get_or_try_init(|| self.warm_lane(&lane)).await {
            lane.active.fetch_sub(1, Ordering::Relaxed);
            return Err(e);
        }
        match self.spawn_dial_stdio(&lane) {
            Ok((child, io)) => Ok(Upstream { child, io, _permit: permit, lane }),
            Err(e) => {
                lane.active.fetch_sub(1, Ordering::Relaxed);
                Err(e)
            }
        }
    }

    /// Tear down every master connection (`ssh -O exit`) and remove control sockets.
    pub async fn shutdown(&self) {
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
pub struct Upstream {
    child: Child,
    io: Join<ChildStdout, ChildStdin>,
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
// Async proxy
// ----------------------------------------------------------------------------

/// The unix-socket proxy. Accepts local connections and bridges each to a warm
/// pooled channel. Usually you want [`ProxyHandle`] instead, which wraps this in
/// a synchronous API.
pub struct PoolProxy {
    socket_path: PathBuf,
    pool: Arc<Pool>,
    accept_stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl PoolProxy {
    /// Read `DOCKER_HOST`; if it is `ssh://…`, build and start a proxy. Otherwise
    /// return `None` (local/tcp daemons don't need pooling).
    pub async fn maybe_start_from_env() -> Result<Option<Self>> {
        let Some(target) = ssh_target_from_env()? else { return Ok(None) };
        Ok(Some(Self::start(target, opts_from_env()).await?))
    }

    /// Bind the socket, warm the first lane, and start accepting.
    pub async fn start(target: SshTarget, opts: PoolOpts) -> Result<Self> {
        let socket_path = default_socket_path();
        // Remove a stale socket so bind() doesn't fail with EADDRINUSE.
        let _ = std::fs::remove_file(&socket_path);
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

    /// The `DOCKER_HOST` value clients should use.
    pub fn docker_host(&self) -> String {
        format!("unix://{}", self.socket_path.display())
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Stop accepting, tear down the SSH masters, and remove the socket.
    pub async fn shutdown(self) {
        let _ = self.accept_stop.send(true);
        let _ = self.task.await;
        self.pool.shutdown().await;
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Bridge one accepted local connection to a pooled upstream channel.
async fn handle(pool: Arc<Pool>, mut local: UnixStream) {
    let mut up = match pool.dial().await {
        Ok(u) => u,
        Err(e) => {
            warn!("dockerpool: upstream dial failed: {e}");
            return; // dropping `local` closes it
        }
    };
    // copy_bidirectional copies both ways and does the half-close dance moby's
    // dial-stdio does: when one side hits EOF it shuts down the peer's write half
    // (CloseWrite) rather than closing fully, so the other direction can drain.
    match tokio::io::copy_bidirectional(&mut local, &mut up.io).await {
        Ok((to_remote, to_local)) => {
            debug!("dockerpool: session done ({to_remote} up / {to_local} down bytes)")
        }
        Err(e) => debug!("dockerpool: session ended: {e}"),
    }
    up.finish().await;
}

// ----------------------------------------------------------------------------
// Synchronous handle for a (sync) cargo-green main
// ----------------------------------------------------------------------------

/// Fully synchronous front door. Runs the async proxy on a dedicated thread and
/// returns once the socket is bound and the first lane is warm.
///
/// ```ignore
/// // In the top-level `cargo green` command, BEFORE spawning cargo:
/// let proxy = crate::docker_pool::ProxyHandle::maybe_start_from_env()?;
/// let mut cargo = std::process::Command::new("cargo");
/// cargo.args(forwarded_args);
/// if let Some(p) = &proxy {
///     // Tell the rustc-wrapper subprocesses where the pool is. They inherit it
///     // through cargo and redirect their `docker` child via pooled_docker_host().
///     cargo.env(crate::docker_pool::POOL_SOCK_ENV, p.socket_path());
/// }
/// let status = cargo.status()?;
/// drop(proxy); // or proxy.shutdown(): stops accepting + `ssh -O exit`
/// std::process::exit(status.code().unwrap_or(1));
/// ```
pub struct ProxyHandle {
    socket_path: PathBuf,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl ProxyHandle {
    /// `Ok(None)` when `DOCKER_HOST` isn't `ssh://…` (nothing to pool).
    pub fn maybe_start_from_env() -> Result<Option<Self>> {
        let Some(target) = ssh_target_from_env()? else { return Ok(None) };
        Ok(Some(Self::start(target, opts_from_env())?))
    }

    pub fn start(target: SshTarget, opts: PoolOpts) -> Result<Self> {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<PathBuf>>();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let join =
            std::thread::Builder::new().name("supergreen-dockerpool".into()).spawn(move || {
                let rt = match tokio::runtime::Runtime::new() {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.into()));
                        return;
                    }
                };
                rt.block_on(async move {
                    let proxy = match PoolProxy::start(target, opts).await {
                        Ok(p) => p,
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                            return;
                        }
                    };
                    let _ = ready_tx.send(Ok(proxy.socket_path().to_path_buf()));
                    // Serve until the sync side drops or shuts down the handle.
                    let _ = shutdown_rx.await;
                    proxy.shutdown().await;
                });
            })?;

        match ready_rx.recv() {
            Ok(Ok(socket_path)) => {
                Ok(Self { socket_path, shutdown_tx: Some(shutdown_tx), join: Some(join) })
            }
            Ok(Err(e)) => {
                let _ = join.join();
                Err(e)
            }
            Err(_) => Err(anyhow!("dockerpool: proxy thread exited before ready")),
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn docker_host(&self) -> String {
        format!("unix://{}", self.socket_path.display())
    }

    /// Explicit, blocking shutdown. (Drop does the same if you don't call it.)
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()); // oneshot::Sender::send is synchronous
        }
        if let Some(j) = self.join.take() {
            let _ = j.join(); // waits for `ssh -O exit` + socket cleanup
        }
    }
}

impl Drop for ProxyHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

// ----------------------------------------------------------------------------
// Shared helpers
// ----------------------------------------------------------------------------

fn ssh_target_from_env() -> Result<Option<SshTarget>> {
    let host = match std::env::var("DOCKER_HOST") {
        Ok(h) => h,
        Err(_) => return Ok(None),
    };
    if !host.starts_with("ssh://") {
        return Ok(None);
    }
    parse_ssh_url(&host)
        .map(Some)
        .ok_or_else(|| anyhow!("dockerpool: bad ssh DOCKER_HOST {host:?}"))
}

fn opts_from_env() -> PoolOpts {
    let mut opts = PoolOpts::default();
    if let Ok(cmd) = std::env::var("CARGOGREEN_DIAL_STDIO_CMD") {
        let parts: Vec<String> = cmd.split_whitespace().map(String::from).collect();
        if !parts.is_empty() {
            opts.remote_cmd = parts;
        }
    }
    if let Some(n) = std::env::var("CARGOGREEN_POOL_MAX_LANES").ok().and_then(|s| s.parse().ok()) {
        opts.max_lanes = n;
    }
    if let Some(n) = std::env::var("CARGOGREEN_POOL_MAX_PER_LANE").ok().and_then(|s| s.parse().ok())
    {
        opts.max_per_lane = n;
    }
    opts
}

fn default_socket_path() -> PathBuf {
    // XDG_RUNTIME_DIR (e.g. /run/user/1000) is short and tmpfs-backed on Linux.
    // Falls back to the temp dir elsewhere. NOTE on macOS: the temp dir path can
    // be long and unix sockets are capped (~104 bytes); set a short $TMPDIR if so.
    let dir =
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join(format!("supergreen-docker-{}.sock", std::process::id()))
}

#[cfg(test)]
mod tests {
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
}
