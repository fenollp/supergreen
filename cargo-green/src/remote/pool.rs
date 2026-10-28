//! Warm, multiplexed connections to the remote daemon.
//!
//! Compiling each crate is >=1 Runner execution. When using a remote runner (with eg.
//! `DOCKER_HOST=ssh://host` or `BUILDKIT_HOST=ssh://host` and such), each of these
//! spawns its own `ssh … docker system dial-stdio` (or equivalent) paying a full
//! SSH handshake (TCP + key exchange + auth) per connection.
//!
//! Here instead the long-lived `cargo green` process owns a pool of (gRPC) connections
//! and serves them on a local UNIX Domain Socket. Wrappers (separate processes spawned by cargo)
//! derive the socket from their [`Green`](crate::green::Green) settings and point the runner at it:
//!
//! ```text
//!   `cargo green` reads DOCKER_HOST=ssh://big-muscle.machine
//!     ├── creates (owns) the pool and sets Green setting
//!     └── `cargo`
//!          └── `cargo-green` ($RUSTC_WRAPPER, per crate)
//!                └── DOCKER_HOST=unix://$XDG_RUNTIME_DIR/cargo-green/pool-<hash>.sock `docker buildx build …`
//! ```
//!
//! Mirroring moby's `docker system dial-stdio` (cli/command/system/dial_stdio.go)
//! and BuildKit's ssh connection helper (client/connhelper/ssh):
//! * Upstream channels are opened through [`super::ssh`]:
//!   only a few `ControlMaster` connections pay a handshake.
//! * A few such channels are kept connected ahead of time ("spares"),
//!   so handing one out costs no process startup nor handshake.
//! * Each accepted connection is bridged as a transparent byte pipe, half-closes included:
//!   HTTP/1.1 REST, hijacked exec streams and buildx's HTTP/2 gRPC all pass through untouched.
//!
//! The socket path hash only depends on the remote addr, as buildx records a builder's endpoint verbatim.
//! Concurrent `cargo green` processes targeting the same host share one pool:
//! the first one serves it, the others connect to it (or serve it, if it never answers).
//! The owner waits for these others to be done before tearing the pool down.

use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions, TryLockError},
    iter,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt, copy},
    net::{UnixListener, UnixStream},
    process::Child,
    sync::{OwnedSemaphorePermit, Semaphore, watch},
    task::{JoinHandle, spawn_blocking},
    time::{sleep, timeout},
};
use url::Url;

use crate::{
    PKG,
    dirs::{Dirs, hash},
    remote::{
        RemoteDaemon,
        ssh::{self, LaneSlot, Lanes},
    },
    runner::Runner,
};

/// Connected channels kept ready to be handed out
const SPARES: usize = 4;

/// Unix socket paths are capped (`sun_path`): 108 bytes on Linux, 104 on macOS.
const MAX_SOCKET_PATH: usize = 104;

/// How long to wait on another process' pool to answer
const JOIN_TIMEOUT: Duration = Duration::from_secs(20);

/// Handle on a (possibly shared) pool. Dropping it without calling [`RemotePool::shutdown`]
/// leaves sockets behind, which the next pool owner cleans up.
pub(crate) struct RemotePool {
    socket: Utf8PathBuf,
    /// Shared lock held while we may use the socket.
    users: File,
    /// Set when this process serves the pool.
    owned: Option<Owned>,
}

struct Owned {
    /// Exclusive lock held while serving.
    _owner: File,
    pool: Arc<Pool>,
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl RemoteDaemon {
    /// Builds reach `ssh://` daemons through our pool, with docker.
    fn is_pooled(&self, runner: Runner) -> bool {
        runner == Runner::Docker && self.is_poolable()
    }

    fn pool_name(&self) -> String {
        format!("pool-{}", hash(&self.key()))
    }

    /// Where our pool to this daemon listens, when builds go through one.
    ///
    /// Odd characters in the socket path get percent-encoded, which docker and BuildKit decode back.
    #[must_use]
    pub(crate) fn pooled(&self, runner: Runner, dirs: Option<&Dirs>) -> Option<Pooled> {
        let Dirs { var_run, .. } = dirs.filter(|_| self.is_pooled(runner))?;
        let path = var_run.join(format!("{}.sock", self.pool_name()));
        let addr = Url::parse(&format!("unix://{path}")).ok()?;
        Some(Pooled { var: self.var().to_owned(), addr })
    }
}

/// Builds go through our pool: the variable holding the daemon's address,
/// and our pool's address to set it to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Pooled {
    pub(crate) var: String,
    pub(crate) addr: Url,
}

impl RemotePool {
    /// Starts or joins a pool when builds go through one, see [`RemoteDaemon::pooled`].
    pub(crate) async fn maybe_start(
        remote: Option<&RemoteDaemon>,
        runner: Runner,
        dirs: Option<&Dirs>,
    ) -> Result<Option<Self>> {
        let Some(remote) = remote.filter(|r| r.is_pooled(runner)) else { return Ok(None) };
        let Some(Dirs { var_run, .. }) = dirs else {
            info!("remote_pool: no runtime directory: not pooling");
            return Ok(None);
        };
        Ok(Some(Self::start(remote, var_run).await?))
    }

    async fn start(remote: &RemoteDaemon, dir: &Utf8Path) -> Result<Self> {
        let name = remote.pool_name();
        let socket = dir.join(format!("{name}.sock"));
        if socket.as_str().len() >= MAX_SOCKET_PATH {
            bail!("Socket path too long, try a shorter $XDG_RUNTIME_DIR or $HOME: {socket}")
        }
        let mut lanes = Some(Lanes::new(remote, dir, &name));

        // Blocks while a previous owner waits for its users to leave
        let users = lock_shared(dir.join(format!("{name}.users"))).await?;

        // Serve the pool, or join the process serving it.
        // That one may never answer (e.g. its handshake failed): then try serving it ourselves.
        let owner = open_lockfile(&dir.join(format!("{name}.owner")))?;
        let deadline = Instant::now() + JOIN_TIMEOUT;
        loop {
            match owner.try_lock() {
                Ok(()) => {
                    let lanes = lanes.take().expect("PROOF: returns right after");
                    let pool = Arc::new(Pool::new(lanes));
                    let (stop, task) = serve(pool.clone(), &socket).await?;
                    info!("remote_pool: serving unix://{socket}");
                    let owned = Some(Owned { _owner: owner, pool, stop, task });
                    return Ok(Self { socket, users, owned });
                }
                Err(TryLockError::WouldBlock) => {
                    // Its owner binds the socket before connecting upstream: this does not wait on it
                    if UnixStream::connect(&socket).await.is_ok() {
                        info!("remote_pool: joining unix://{socket}");
                        return Ok(Self { socket, users, owned: None });
                    }
                    if Instant::now() > deadline {
                        bail!("Pool at {socket} is owned by another process but not answering")
                    }
                    sleep(Duration::from_millis(100)).await;
                }
                Err(TryLockError::Error(e)) => bail!("Failed locking pool owner file: {e}"),
            }
        }
    }

    /// Owner: waits for other users then stops serving and removes the socket.
    pub(crate) async fn shutdown(self) {
        let Self { socket, users, owned } = self;
        let Some(Owned { _owner, pool, stop, task }) = owned else { return };

        // Trade our shared lock for an exclusive one: blocks while others still use the socket.
        let _users = spawn_blocking(move || {
            let _ = users.unlock();
            if let Err(TryLockError::WouldBlock) = users.try_lock() {
                info!("remote_pool: waiting on other {PKG} processes using the pool");
                let _ = users.lock();
            }
            users
        })
        .await;

        let _ = stop.send(true);
        let _ = task.await;
        let _ = fs::remove_file(&socket);
        pool.shutdown();
    }
}

async fn serve(
    pool: Arc<Pool>,
    socket: &Utf8Path,
) -> Result<(watch::Sender<bool>, JoinHandle<()>)> {
    let _ = fs::remove_file(socket); // Stale socket from a dead owner
    let listener =
        UnixListener::bind(socket).map_err(|e| anyhow!("Failed binding pool on {socket}: {e}"))?;
    // Owner-only: this socket is a direct line to the (remote) daemon
    fs::set_permissions(socket, fs::Permissions::from_mode(0o600))?;

    // Pay the first handshake now and get spares going
    pool.prewarm().await?;

    let (stop, mut stopped) = watch::channel(false);
    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = stopped.changed() => break,
                accepted = listener.accept() => match accepted {
                    Ok((conn, _)) => {
                        tokio::spawn(handle(pool.clone(), conn));
                    }
                    Err(e) => {
                        warn!("remote_pool: accept failed: {e}");
                        break;
                    }
                },
            }
        }
    });
    Ok((stop, task))
}

fn open_lockfile(path: &Utf8Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(|e| anyhow!("Failed opening {path}: {e}"))
}

async fn lock_shared(path: Utf8PathBuf) -> Result<File> {
    spawn_blocking(move || {
        let file = open_lockfile(&path)?;
        file.lock_shared().map_err(|e| anyhow!("Failed locking {path}: {e}"))?;
        Ok(file)
    })
    .await?
}

/// Bridge one accepted local connection to an upstream channel, dial-stdio style
/// (docker/cli's cli/command/system/dial_stdio.go, as of b774e75):
/// EOF on one side closes the write half of the other.
async fn handle(pool: Arc<Pool>, local: UnixStream) {
    let Upstream { link: Link { mut read, mut write, ssh, .. }, permit } = match pool.dial().await {
        Ok(up) => up,
        Err(e) => {
            warn!("remote_pool: dial failed: {e}");
            return; // Dropping local closes it
        }
    };
    let (mut local_r, mut local_w) = local.into_split();

    let to_remote = async move {
        let res = copy(&mut local_r, &mut write).await;
        let _ = write.shutdown().await;
        drop(write); // Closes ssh's stdin
        res
    };
    let to_local = async move {
        let res = copy(&mut read, &mut local_w).await;
        let _ = local_w.shutdown().await;
        res
    };

    // As dial-stdio: once the daemon is done talking, the session is over
    // without waiting for the local side to close its writes.
    tokio::pin!(to_remote, to_local);
    let res = tokio::select! {
        up = &mut to_remote => match up {
            Ok(_) => to_local.await,
            Err(e) => Err(e),
        },
        down = &mut to_local => down,
    };
    match res {
        Ok(down) => debug!("remote_pool: session done ({down} bytes down)"),
        Err(e) => debug!("remote_pool: session ended: {e}"),
    }

    if let Some((mut child, slot)) = ssh {
        if timeout(Duration::from_secs(5), child.wait()).await.is_err() {
            let _ = child.kill().await;
        }
        drop(slot);
    }
    drop(permit); // Only now is the channel gone
}

/// A connected channel to the remote daemon.
pub(super) struct Link {
    read: Box<dyn AsyncRead + Send + Unpin>,
    write: Box<dyn AsyncWrite + Send + Unpin>,
    /// The process carrying the channel and its lane accounting
    ssh: Option<(Child, LaneSlot)>,
}

impl Link {
    pub(super) fn new(
        read: impl AsyncRead + Send + Unpin + 'static,
        write: impl AsyncWrite + Send + Unpin + 'static,
    ) -> Self {
        Self { read: Box::new(read), write: Box::new(write), ssh: None }
    }

    pub(super) fn with_child(self, child: Child, slot: LaneSlot) -> Self {
        Self { ssh: Some((child, slot)), ..self }
    }

    fn is_alive(&mut self) -> bool {
        let running = |(child, _): &mut (Child, _)| matches!(child.try_wait(), Ok(None));
        self.ssh.as_mut().is_none_or(running)
    }
}

/// A channel, holding its semaphore permit for its whole lifetime.
struct Upstream {
    link: Link,
    permit: OwnedSemaphorePermit,
}

struct Pool {
    lanes: Lanes,
    /// Bounds live channels (spares included) to `MAX_CHANNELS`,
    /// which guarantees a free lane slot for any permit holder.
    sem: Arc<Semaphore>,
    spares: Mutex<VecDeque<Upstream>>,
    /// Set while a task tops up spares
    refilling: AtomicBool,
}

impl Pool {
    fn new(lanes: Lanes) -> Self {
        Self {
            lanes,
            sem: Arc::new(Semaphore::new(ssh::MAX_CHANNELS)),
            spares: Mutex::new(VecDeque::new()),
            refilling: AtomicBool::new(false),
        }
    }

    async fn prewarm(self: &Arc<Self>) -> Result<()> {
        self.open().await.map(|up| self.spares.lock().unwrap().push_back(up))?;
        self.to_owned().refill();
        Ok(())
    }

    async fn open(&self) -> Result<Upstream> {
        let permit = self.sem.clone().acquire_owned().await.context("remote_pool: closed")?;
        self.open_with(permit).await
    }

    async fn open_with(&self, permit: OwnedSemaphorePermit) -> Result<Upstream> {
        let link = self.lanes.open().await?;
        Ok(Upstream { link, permit })
    }

    /// Hands out a ready spare if any, otherwise opens a channel.
    async fn dial(self: &Arc<Self>) -> Result<Upstream> {
        let spare = {
            let mut spares = self.spares.lock().unwrap();
            iter::from_fn(|| spares.pop_front()).find_map(|mut up| up.link.is_alive().then_some(up))
        };
        self.clone().refill();
        match spare {
            Some(up) => Ok(up),
            None => self.open().await,
        }
    }

    /// Tops up spares in the background, without competing with live channels for permits.
    fn refill(self: Arc<Self>) {
        if self.refilling.swap(true, Ordering::Acquire) {
            return; // Already at it
        }
        tokio::spawn(async move {
            while self.spares.lock().unwrap().len() < SPARES {
                let Ok(permit) = self.sem.clone().try_acquire_owned() else { break };
                match self.open_with(permit).await {
                    Ok(up) => self.spares.lock().unwrap().push_back(up),
                    Err(e) => {
                        warn!("remote_pool: failed opening spare: {e}");
                        break;
                    }
                }
            }
            self.refilling.store(false, Ordering::Release);
        });
    }

    /// Closes spares. ssh masters linger for `ControlPersist` so the next run skips the handshake.
    fn shutdown(&self) {
        self.sem.close();
        self.spares.lock().unwrap().clear();
    }
}

/// `DOCKER_HOST=ssh://… cargo nextest run --run-ignored=only -E 'test(/e2e::(pool_|pooled_docker)/)'`
/// `BUILDKIT_HOST=ssh://… cargo nextest run --run-ignored=only remote::pool::e2e::pooled_buildkit`
#[cfg(test)]
mod sockets {
    use url::Url;

    use crate::{dirs::Dirs, green::Green, redact::Vars, remote::RemoteDaemon, runner::Runner};

    fn dirs_at(var_run: &str) -> Dirs {
        Dirs {
            tmp: "/t".into(),
            results: "/r".into(),
            buildkit: "/b".into(),
            var_run: var_run.into(),
        }
    }

    fn dirs() -> Dirs {
        dirs_at("/run/cg")
    }

    fn addr(addr: &str, runner: Runner, dirs: Option<&Dirs>) -> Option<Url> {
        let remote = RemoteDaemon::from_env(&Vars::from([(DOCKER_HOST!(), addr)])).unwrap();
        let pooled = remote.unwrap().pooled(runner, dirs)?;
        assert_eq!(pooled.var, DOCKER_HOST!());
        Some(pooled.addr)
    }

    fn socket(remote: &str, runner: Runner, dirs: Option<&Dirs>) -> Option<String> {
        addr(remote, runner, dirs).map(|url| url.path().to_owned())
    }

    #[test]
    fn pooled_with_docker() {
        let got = socket("ssh://h", Runner::Docker, Some(&dirs())).unwrap();
        assert!(got.starts_with("/run/cg/pool-") && got.ends_with(".sock"), "{got}");
    }

    #[test]
    fn pool_addrs() {
        let url = addr("ssh://h", Runner::Docker, Some(&dirs())).unwrap();
        assert_eq!(url.scheme(), "unix");
        assert_eq!(url.as_str(), format!("unix://{}", url.path()));

        let url = addr("ssh://h", Runner::Docker, Some(&dirs_at("/run/c g"))).unwrap();
        assert!(url.as_str().starts_with("unix:///run/c%20g/pool-"), "{url}");
    }

    #[test]
    fn same_daemon_same_socket() {
        let got = |addr| socket(addr, Runner::Docker, Some(&dirs()));
        assert_eq!(got("ssh://h"), got("ssh://h/"));
        assert_ne!(got("ssh://h"), got("ssh://g"));
    }

    #[test]
    fn passed_to_wrappers() {
        let remote = RemoteDaemon::from_env(&Vars::from([(DOCKER_HOST!(), "ssh://h")])).unwrap();
        let pool = remote.unwrap().pooled(Runner::Docker, Some(&dirs()));
        assert!(pool.is_some());
        let green = Green { pool: pool.clone(), ..Green::default() };

        let json = serde_json::to_string(&green).unwrap();
        assert_eq!(serde_json::from_str::<Green>(&json).unwrap().pool, pool);
    }

    #[test]
    fn not_pooled() {
        assert_eq!(socket("unix:///var/run/docker.sock", Runner::Docker, Some(&dirs())), None);
        assert_eq!(socket("tcp://h:2375", Runner::Docker, Some(&dirs())), None);
        assert_eq!(socket("ssh://h", Runner::Podman, Some(&dirs())), None);
        assert_eq!(socket("ssh://h", Runner::None, Some(&dirs())), None);
        assert_eq!(socket("ssh://h", Runner::Docker, None), None);
    }
}

#[cfg(test)]
mod e2e {
    use std::{
        fs,
        process::Stdio,
        time::{Duration, Instant},
    };

    use camino::{Utf8Path, Utf8PathBuf};
    use tokio::{
        io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, join},
        net::UnixStream,
        process::Command,
        time::sleep,
    };
    use url::Url;

    use super::{RemotePool, SPARES, open_lockfile};
    use crate::{
        PKG,
        dirs::hash,
        redact::Vars,
        remote::{RemoteDaemon, ssh},
        sys::Sys,
    };

    fn env() -> Vars {
        std::env::vars().collect()
    }

    fn remote(var: &str) -> (RemoteDaemon, Utf8PathBuf) {
        let remote = RemoteDaemon::from_env(&env()).unwrap().filter(RemoteDaemon::is_poolable);
        let remote = remote.expect("set *_HOST=ssh://…");
        assert_eq!(remote.var(), var);
        // Not dirs::setup_dirs(): tests run without side effects
        let dir = std::env::var("XDG_RUNTIME_DIR").expect("set $XDG_RUNTIME_DIR");
        let dir = Utf8PathBuf::from(dir).join(PKG);
        fs::create_dir_all(&dir).unwrap();
        (remote, dir)
    }

    async fn start_pool(var: &str) -> (RemotePool, RemoteDaemon, String) {
        let (remote, dir) = self::remote(var);
        let (pool, pooled) = start_at(&remote, &dir).await;
        (pool, remote, pooled)
    }

    async fn start_at(remote: &RemoteDaemon, dir: &Utf8Path) -> (RemotePool, String) {
        let pool = RemotePool::start(remote, dir).await.unwrap();
        let pooled = format!("unix://{}", pool.socket);
        (pool, pooled)
    }

    async fn docker_version(host: &str) {
        let out = Command::new("docker")
            .args(["version", "--format={{.Server.Version}}"])
            .env(DOCKER_HOST!(), host)
            .output()
            .await
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }

    /// Opens an HTTP/2 connection (as gRPC does) and waits for the server's SETTINGS.
    async fn h2_handshake(mut io: impl AsyncRead + AsyncWrite + Unpin) {
        io.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n\0\0\0\x04\0\0\0\0\0").await.unwrap();
        let mut frame = [0; 9];
        io.read_exact(&mut frame).await.unwrap();
        assert_eq!(frame[3], 0x04, "not a SETTINGS frame: {frame:?}");
    }

    async fn buildkit_direct(remote: &RemoteDaemon) {
        let mut child = Command::new("ssh")
            .arg("-T")
            .args(remote.dest())
            .args(remote.dial_stdio())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let (stdin, stdout) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
        h2_handshake(join(stdout, stdin)).await;
    }

    async fn buildkit_pooled(pooled: &str) {
        let socket = Url::parse(pooled).unwrap();
        h2_handshake(UnixStream::connect(socket.path()).await.unwrap()).await;
    }

    async fn avg<F: Future<Output = ()>>(n: u32, mut f: impl FnMut() -> F) -> Duration {
        let start = Instant::now();
        for _ in 0..n {
            f().await;
        }
        start.elapsed() / n
    }

    #[tokio::test]
    #[ignore = "needs DOCKER_HOST=ssh://…"]
    async fn pooled_docker_connections_are_cheaper() {
        let _guard = Sys::install(Sys::real());
        let (pool, _, pooled) = start_pool(DOCKER_HOST!()).await;
        let direct = std::env::var(DOCKER_HOST!()).unwrap();

        let direct = avg(5, || docker_version(&direct)).await;
        let pooled = avg(5, || docker_version(&pooled)).await;
        eprintln!("docker version: direct {direct:?} vs pooled {pooled:?}");
        assert!(pooled * 3 < direct, "direct {direct:?} vs pooled {pooled:?}");

        pool.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "needs BUILDKIT_HOST=ssh://…"]
    async fn pooled_buildkit_connections_are_cheaper() {
        let _guard = Sys::install(Sys::real());
        let (pool, remote, pooled) = start_pool(BUILDKIT_HOST!()).await;

        let direct = avg(5, || buildkit_direct(&remote)).await;
        let pooled = avg(5, || buildkit_pooled(&pooled)).await;
        eprintln!("buildkit gRPC connection: direct {direct:?} vs pooled {pooled:?}");
        assert!(pooled * 3 < direct, "direct {direct:?} vs pooled {pooled:?}");

        pool.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs DOCKER_HOST=ssh://…"]
    async fn pool_handles_concurrency() {
        let _guard = Sys::install(Sys::real());
        let (pool, _, pooled) = start_pool(DOCKER_HOST!()).await;

        // More than a lane holds, fewer than all lanes do
        let n = 3 * ssh::MAX_PER_LANE;
        let start = Instant::now();
        let calls: Vec<_> = (0..n)
            .map(|_| {
                let pooled = pooled.clone();
                tokio::spawn(async move { avg(3, || docker_version(&pooled)).await })
            })
            .collect();
        for call in calls {
            call.await.unwrap();
        }
        eprintln!("{} concurrent docker version calls in {:?}", 3 * n, start.elapsed());

        // Spread over lanes, without opening more than needed
        let lanes = pool.owned.as_ref().unwrap().pool.lanes.count();
        let most = (n + SPARES).div_ceil(ssh::MAX_PER_LANE);
        assert!((2..=most).contains(&lanes), "{lanes} lanes");

        pool.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs DOCKER_HOST=ssh://…"]
    async fn pool_taken_over_from_unanswering_owner() {
        let _guard = Sys::install(Sys::real());
        let (remote, dir) = self::remote(DOCKER_HOST!());
        let name = format!("pool-{}", hash(&remote.key()));

        // Some owner that never serves: it holds the lock but no socket answers
        let owner = open_lockfile(&dir.join(format!("{name}.owner"))).unwrap();
        owner.try_lock().unwrap();
        let _ = fs::remove_file(dir.join(format!("{name}.sock")));
        let gone = tokio::spawn(async move {
            sleep(Duration::from_millis(500)).await;
            drop(owner); // Owner gives up (e.g. failed handshake)
        });

        let (pool, pooled) = start_at(&remote, &dir).await;
        gone.await.unwrap();
        assert!(pool.owned.is_some(), "took over");
        docker_version(&pooled).await;
        pool.shutdown().await;
    }
}
