//! Warm, multiplexed connections to the remote daemon builds go to.
//!
//! We run one BuildKit build per `rustc` call by shelling out to `docker buildx build`.
//! With `DOCKER_HOST=ssh://host` each of these spawns its own `ssh … docker system dial-stdio`,
//! paying a full SSH handshake (TCP + key exchange + auth) per connection.
//! A `docker buildx build` opens a few connections, so a workspace pays thousands of handshakes.
//! Same with a `remote` builder (`BUILDKIT_HOST=ssh://host`) and `ssh … buildctl dial-stdio`.
//!
//! Either way it is BuildKit gRPC that flows: `docker-container` builders tunnel it
//! through the Docker API (`docker exec … buildctl dial-stdio`), `remote` ones dial it directly
//! (and never use `$DOCKER_HOST`). So one pool serves whichever daemon builds go to.
//!
//! Here instead the long-lived `cargo green` process owns a pool of SSH connections
//! and serves them on a local unix socket. Wrappers (separate processes spawned by cargo)
//! find the socket in their [`Green`](crate::green::Green) settings and point their `docker` at it:
//!
//! ```text
//!   cargo green (owns the pool)
//!     ├── unix://$XDG_RUNTIME_DIR/cargo-green/pool-<hash>.sock
//!     └── cargo
//!          └── cargo-green (RUSTC_WRAPPER, per crate)
//!                └── DOCKER_HOST=unix://…sock docker buildx build …
//! ```
//!
//! Mirroring moby's `docker system dial-stdio` (cli/command/system/dial_stdio.go)
//! and BuildKit's ssh connection helper (client/connhelper/ssh):
//! * Upstream channels are system `ssh` processes running `docker system dial-stdio`
//!   (or `buildctl dial-stdio`) remotely,
//!   multiplexed over a few `ControlMaster` connections: only these pay a handshake.
//!   Using `ssh` keeps `~/.ssh/config` (aliases, `ProxyJump`, `IdentityFile`, ..) working.
//! * A few such channels are kept spawned & connected ahead of time ("spares"),
//!   so handing one out costs no process startup, on either side.
//! * Each accepted connection is bridged as a transparent byte pipe, half-closes included:
//!   HTTP/1.1 REST, hijacked exec streams and buildx's HTTP/2 gRPC all pass through untouched.
//! * Channels are spread over "lanes" (masters) to stay under sshd's `MaxSessions` (default: 10).
//!
//! The socket path only depends on the remote, as buildx records a builder's endpoint verbatim.
//! Concurrent `cargo green` processes targeting the same host share one pool:
//! the first one serves it, the others connect to it.
//! The owner waits for these others to be done before tearing the pool down.

use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    iter,
    os::unix::fs::PermissionsExt,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info, warn};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, copy},
    net::{UnixListener, UnixStream},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{OnceCell, OwnedSemaphorePermit, Semaphore, watch},
    task::{JoinHandle, spawn_blocking},
    time::{sleep, timeout},
};
use url::{Host, Url};

use crate::{PKG, builder::Daemon, dirs::hash, redact::Vars, runner::Runner};

/// Most SSH master connections (each costs one handshake)
const MAX_LANES: usize = 8;
/// Most channels per master: under sshd's `MaxSessions` (OpenSSH default: 10)
const MAX_PER_LANE: usize = 8;
/// Connected channels kept ready to be handed out
const SPARES: usize = 4;

/// Unix socket paths are capped (`sun_path`): 108 bytes on Linux, 104 on macOS.
const MAX_SOCKET_PATH: usize = 104;

/// A daemon reachable through ssh.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Remote {
    var: &'static str,
    url: Url,
}

impl Remote {
    fn from_env(env: &Vars) -> Result<Option<Self>> {
        let Some(Daemon { var, addr, url }) =
            env.builder_daemon()?.filter(|d| d.url.scheme() == "ssh")
        else {
            return Ok(None);
        };
        if url.host().is_none() {
            bail!("Missing host in ${var}={addr:?}")
        }
        Ok(Some(Self { var, url }))
    }

    /// Remote command bridging a channel's stdio to the daemon
    fn dial_stdio(&self) -> Vec<String> {
        if self.var == DOCKER_HOST!() {
            return ["docker", "system", "dial-stdio"].map(ToOwned::to_owned).to_vec();
        }
        // Same as BuildKit's ssh connection helper
        let mut cmd = vec!["buildctl".to_owned()];
        if !matches!(self.url.path(), "" | "/") {
            cmd.extend(["--addr".to_owned(), format!("unix://{}", self.url.path())]);
        }
        cmd.push("dial-stdio".to_owned());
        cmd
    }

    /// Args selecting this destination, for after the ssh options.
    fn dest(&self) -> Vec<String> {
        let mut args = vec![];
        if !self.url.username().is_empty() {
            args.extend(["-l".to_owned(), self.url.username().to_owned()]);
        }
        if let Some(port) = self.url.port() {
            args.extend(["-p".to_owned(), port.to_string()]);
        }
        let host = match self.url.host() {
            Some(Host::Ipv6(ip)) => ip.to_string(), // Without brackets
            host => host.expect("checked").to_string(),
        };
        args.extend(["--".to_owned(), host]);
        args
    }
}

#[test]
fn remotes() {
    let remote = |envs: &[(&str, &str)]| {
        let env = Vars::from(envs);
        Remote::from_env(&env).map(|r| r.map(|r| (r.dest().join(" "), r.dial_stdio().join(" "))))
    };
    let some = |dest: &str, dial: &str| Some((dest.to_owned(), dial.to_owned()));

    assert_eq!(remote(&[]).unwrap(), None);
    assert_eq!(remote(&[(DOCKER_HOST!(), "unix:///var/run/docker.sock")]).unwrap(), None);
    assert_eq!(
        remote(&[(DOCKER_HOST!(), "ssh://ter")]).unwrap(),
        some("-- ter", "docker system dial-stdio")
    );
    assert_eq!(
        remote(&[(DOCKER_HOST!(), "ssh://user@[::1]:22/")]).unwrap(),
        some("-l user -p 22 -- ::1", "docker system dial-stdio")
    );
    assert!(remote(&[(DOCKER_HOST!(), "ssh://ter"), (BUILDKIT_HOST!(), "ssh://ter")]).is_err());
    assert_eq!(
        remote(&[(DOCKER_HOST!(), ""), (BUILDKIT_HOST!(), "tcp://ter:1234")]).unwrap(),
        None
    );
    assert_eq!(
        remote(&[(BUILDKIT_HOST!(), "ssh://ter")]).unwrap(),
        some("-- ter", "buildctl dial-stdio")
    );
    assert_eq!(
        remote(&[(BUILDKIT_HOST!(), "ssh://me@ter:2222/run/buildkit/buildkitd.sock")]).unwrap(),
        some(
            "-l me -p 2222 -- ter",
            "buildctl --addr unix:///run/buildkit/buildkitd.sock dial-stdio"
        )
    );
    assert!(remote(&[(BUILDKIT_HOST!(), "ssh://")]).is_err());
    assert!(remote(&[(BUILDKIT_HOST!(), "ssh://host:port")]).is_err());
}

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

impl RemotePool {
    /// Starts or joins a pool when builds go to an ssh daemon, if the runner is docker.
    pub(crate) async fn maybe_start(env: &Vars) -> Result<Option<Self>> {
        let runner = env.get(CARGOGREEN_RUNNER!()).map_or(Ok(Runner::default()), |r| r.parse())?;
        if runner != Runner::Docker {
            return Ok(None);
        }
        let Some(remote) = Remote::from_env(env)? else { return Ok(None) };
        Self::start(remote, &runtime_dir(env)?).await.map(Some)
    }

    /// Address to use in place of the daemon's.
    #[must_use]
    pub(crate) fn addr(&self) -> String {
        format!("unix://{}", self.socket)
    }

    async fn start(remote: Remote, dir: &Utf8Path) -> Result<Self> {
        let name = format!("pool-{}", hash(&format!("{}={}", remote.var, remote.url)));
        let socket = dir.join(format!("{name}.sock"));
        if socket.as_str().len() >= MAX_SOCKET_PATH {
            bail!("Socket path too long, try a shorter $XDG_RUNTIME_DIR or $TMPDIR: {socket}")
        }

        // Blocks while a previous owner waits for its users to leave
        let users = lock_shared(dir.join(format!("{name}.users"))).await?;

        let owner = open_lockfile(&dir.join(format!("{name}.owner")))?;
        let owned = match owner.try_lock() {
            Ok(()) => {
                let pool = Arc::new(Pool::new(remote, dir, &name));
                let (stop, task) = serve(pool.clone(), &socket).await?;
                info!("remote_pool: serving unix://{socket}");
                Some(Owned { _owner: owner, pool, stop, task })
            }
            Err(fs::TryLockError::WouldBlock) => {
                info!("remote_pool: joining unix://{socket}");
                wait_for_socket(&socket).await?;
                None
            }
            Err(fs::TryLockError::Error(e)) => bail!("Failed locking pool owner file: {e}"),
        };

        Ok(Self { socket, users, owned })
    }

    /// Owner: waits for other users then stops serving and removes the socket.
    pub(crate) async fn shutdown(self) {
        let Self { socket, users, owned } = self;
        let Some(Owned { _owner, pool, stop, task }) = owned else { return };

        // Trade our shared lock for an exclusive one: blocks while others still use the socket.
        let _users = spawn_blocking(move || {
            let _ = users.unlock();
            if let Err(fs::TryLockError::WouldBlock) = users.try_lock() {
                eprintln!("Waiting on other {PKG} processes using the connection pool...");
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

async fn wait_for_socket(socket: &Utf8Path) -> Result<()> {
    for _ in 0..200 {
        if UnixStream::connect(socket).await.is_ok() {
            return Ok(());
        }
        sleep(Duration::from_millis(100)).await;
    }
    bail!("Pool at {socket} is owned by another process but not answering")
}

/// Where to put our sockets: short and private.
fn runtime_dir(env: &Vars) -> Result<Utf8PathBuf> {
    // XDG_RUNTIME_DIR (e.g. /run/user/1000) is short, private and tmpfs-backed on Linux.
    let dir = match env.get("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => Utf8PathBuf::from(dir).join(PKG),
        _ => crate::dirs::tmp().join(format!("{PKG}-sockets")),
    };
    fs::create_dir_all(&dir).map_err(|e| anyhow!("Failed creating {dir}: {e}"))?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
        .map_err(|e| anyhow!("Failed restricting {dir}: {e}"))?;
    Ok(dir)
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

/// Bridge one accepted local connection to an upstream channel, dial-stdio style:
/// EOF on one side closes the write half of the other.
async fn handle(pool: Arc<Pool>, local: UnixStream) {
    let Upstream { mut child, stdin, stdout, permit, lane } = match pool.dial().await {
        Ok(up) => up,
        Err(e) => {
            warn!("remote_pool: dial failed: {e}");
            return; // Dropping local closes it
        }
    };
    let (mut local_r, mut local_w) = local.into_split();

    let to_remote = async move {
        let mut stdin = stdin;
        let res = copy(&mut local_r, &mut stdin).await;
        drop(stdin); // ssh forwards EOF, remote dial-stdio then does CloseWrite
        res
    };
    let to_local = async move {
        let mut stdout = stdout;
        let res = copy(&mut stdout, &mut local_w).await;
        let _ = local_w.shutdown().await;
        res
    };

    match tokio::try_join!(to_remote, to_local) {
        Ok((up, down)) => debug!("remote_pool: session done ({up} up / {down} down bytes)"),
        Err(e) => debug!("remote_pool: session ended: {e}"),
    }

    if timeout(Duration::from_secs(5), child.wait()).await.is_err() {
        let _ = child.kill().await;
    }
    drop((permit, lane)); // Only now is the channel gone
}

/// One SSH master connection and its live-channel count.
struct Lane {
    id: usize,
    control_path: Utf8PathBuf,
    active: AtomicUsize,
    warmed: OnceCell<()>,
}

/// Accounts for a channel on a lane.
struct LaneSlot(Arc<Lane>);

impl Drop for LaneSlot {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A connected channel to the remote daemon.
/// Holds its lane slot and semaphore permit for its whole lifetime.
struct Upstream {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    permit: OwnedSemaphorePermit,
    lane: LaneSlot,
}

impl Upstream {
    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

struct Pool {
    remote: Remote,
    dir: Utf8PathBuf,
    name: String,
    /// Bounds live channels (spares included) to `MAX_LANES * MAX_PER_LANE`,
    /// which guarantees a free lane slot for any permit holder.
    sem: Arc<Semaphore>,
    lanes: Mutex<Vec<Arc<Lane>>>,
    spares: Mutex<VecDeque<Upstream>>,
    refilling: tokio::sync::Mutex<()>,
}

impl Pool {
    fn new(remote: Remote, dir: &Utf8Path, name: &str) -> Self {
        Self {
            remote,
            dir: dir.to_owned(),
            name: name.to_owned(),
            sem: Arc::new(Semaphore::new(MAX_LANES * MAX_PER_LANE)),
            lanes: Mutex::new(Vec::new()),
            spares: Mutex::new(VecDeque::new()),
            refilling: tokio::sync::Mutex::new(()),
        }
    }

    /// ssh options selecting (or creating) a lane's master.
    fn ssh(&self, lane: &Lane) -> Command {
        let mut cmd = Command::new("ssh");
        cmd.kill_on_drop(true);
        cmd.arg("-T"); // No pty: raw bytes
        for opt in [
            "ControlMaster=auto".to_owned(),
            format!("ControlPath={}", lane.control_path),
            // Masters linger a bit for the next run, then go away (even if we crash)
            "ControlPersist=120".to_owned(),
            "ConnectTimeout=30".to_owned(),
            "ServerAliveInterval=30".to_owned(),
            "ServerAliveCountMax=3".to_owned(),
        ] {
            cmd.arg("-o").arg(opt);
        }
        cmd
    }

    /// Establishes a lane's master connection: one handshake.
    async fn warm(&self, lane: &Lane) -> Result<()> {
        let mut cmd = self.ssh(lane);
        cmd.args(self.remote.dest()).arg("true");
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
        let out = cmd.output().await.map_err(|e| anyhow!("Failed spawning ssh: {e}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            bail!("Failed to ssh into {}: {stderr}", self.remote.url)
        }
        debug!("remote_pool: lane {} up ({})", lane.id, lane.control_path);
        Ok(())
    }

    async fn prewarm(self: &Arc<Self>) -> Result<()> {
        self.open().await.map(|up| self.spares.lock().unwrap().push_back(up))?;
        self.clone().refill();
        Ok(())
    }

    /// Picks the least-loaded lane with room, creating one when all are full.
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

    async fn open(&self) -> Result<Upstream> {
        let permit = self.sem.clone().acquire_owned().await.context("remote_pool: closed")?;
        self.open_with(permit).await
    }

    /// Opens a channel: cheap once its lane is warm (an SSH session over the existing master).
    async fn open_with(&self, permit: OwnedSemaphorePermit) -> Result<Upstream> {
        let slot = self.choose_lane();
        let lane = &slot.0;
        lane.warmed.get_or_try_init(|| self.warm(lane)).await?;

        let mut cmd = self.ssh(lane);
        cmd.args(self.remote.dest()).args(self.remote.dial_stdio());
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
        Ok(Upstream { child, stdin, stdout, permit, lane: slot })
    }

    /// Hands out a ready spare if any, otherwise opens a channel.
    async fn dial(self: &Arc<Self>) -> Result<Upstream> {
        let spare = {
            let mut spares = self.spares.lock().unwrap();
            iter::from_fn(|| spares.pop_front()).find_map(|mut up| up.is_alive().then_some(up))
        };
        self.clone().refill();
        match spare {
            Some(up) => Ok(up),
            None => self.open().await,
        }
    }

    /// Tops up spares in the background, without competing with live channels for permits.
    fn refill(self: Arc<Self>) {
        tokio::spawn(async move {
            let Ok(_guard) = self.refilling.try_lock() else { return };
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
        });
    }

    /// Closes spares. Masters linger for `ControlPersist` so the next run skips the handshake.
    fn shutdown(&self) {
        self.sem.close();
        self.spares.lock().unwrap().clear();
    }
}

/// `DOCKER_HOST=ssh://… cargo nextest run --run-ignored=only remote_pool`
/// `BUILDKIT_HOST=ssh://… cargo nextest run --run-ignored=only remote_pool::e2e::pooled_buildkit`
#[cfg(test)]
mod e2e {
    use std::time::Instant;

    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, join};

    use super::*;

    async fn start_pool(var: &str) -> (RemotePool, Remote, String) {
        let env: Vars = std::env::vars().collect();
        let remote = Remote::from_env(&env).unwrap().expect("set *_HOST=ssh://…");
        assert_eq!(remote.var, var);
        let pool = RemotePool::start(remote.clone(), &runtime_dir(&env).unwrap()).await.unwrap();
        let pooled = pool.addr();
        (pool, remote, pooled)
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

    async fn buildkit_direct(remote: &Remote) {
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
        let (pool, _, pooled) = start_pool(DOCKER_HOST!()).await;

        // More than a lane holds, fewer than all lanes do
        let n = 3 * MAX_PER_LANE;
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

        pool.shutdown().await;
    }
}
