//! `<cargo green plugin>` <--> many `<cargo green rustc wrappers>`
//!
//! Assumption is server may not always run nor be reliable: wrappers then do without.
//!
//! * serves Md.s from plugin memory and wrappers fallback to disk
//! * wrappers hold a lifeline to the plugin: they stop once it is gone
//! * wrappers tell the plugin what their builds cost, for its report and for replays' to come
//!
//! A wrapper writes an [`Ask`] as a JSON line, the plugin answers with a JSON line
//! whose type depends on the ask. Some asks are just told: no answer.

use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Write},
    mem::take,
    sync::{Arc, Mutex, OnceLock, PoisonError},
    time::Duration,
};

use anyhow::{Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerOptions, Name, Stream, ToFsName, ToNsName,
    tokio::{Listener, Stream as AsyncStream, prelude::*},
    traits::Stream as _,
};
use log::{error, info, warn};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader as AsyncBufReader},
    sync::oneshot,
    task::{JoinHandle, JoinSet},
    time::timeout,
};

use crate::{
    dirs::Paths,
    md::{Md, MdId},
    stats::{BuildStats, ReplayStats},
    sys::{SysFs, fs},
};

/// What a wrapper asks the plugin.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Ask {
    /// Answered with an `Option<Md>`: on `None` the wrapper reads that Md off disk itself.
    ReadMd(Utf8PathBuf),

    /// Answered with `()`: that Md was (re)written, read it afresh next time.
    EvictMd(Utf8PathBuf),
    /// Not answered: that crate was built at that cost, to keep in that file if any. `stored`:
    /// that build wrote the crate's result to the results cache (and did not find it there).
    /// Not answered: that crate was built at that cost, to keep in that file if any.
    Built { this: MdId, stats: BuildStats, file: Option<Utf8PathBuf>, stored: bool },

    /// Not answered: that crate was replayed at that cost, what building it cost being in
    /// that file.
    Replayed { this: MdId, replay: ReplayStats, file: Utf8PathBuf },
}

/// A socket path on Unix, a pipe's name on Windows.
fn name(sock: &str) -> Option<Name<'_>> {
    // Tests use namespaced sockets (abstract ones, on Linux): no files involved
    if cfg!(any(test, windows)) {
        sock.to_ns_name::<GenericNamespaced>()
    } else {
        sock.to_fs_name::<GenericFilePath>()
    }
    .inspect_err(|e| warn!("Bad socket name {sock:?}: {e}"))
    .ok()
}

impl Paths {
    /// Starts chitchatting with wrappers, when we can have a socket.
    pub(crate) fn new_chitchat(&self) -> Option<Server> {
        let file = format!("chitchat-{}.sock", std::process::id());
        #[cfg(windows)]
        let sock = format!("{}-{file}", crate::PKG);
        #[cfg(not(windows))]
        let sock = self.dirs.as_ref()?.var_run.join(file).into_string();
        Server::start(sock).inspect_err(|e| warn!("Not chitchatting: {e}")).ok()
    }

    /// Stops this wrapper as soon as the plugin is gone (crashed, killed, ...)
    ///
    /// Does not steal ^C signals handling for this.
    ///
    /// The lifeline is a task on the current runtime: it goes away with it.
    pub(crate) async fn hold_lifeline(&self) {
        let Some(sock) = self.chitchat_sock.as_deref() else { return };
        let Some(conn) = Client::lifeline(sock).await else { return };
        drop(tokio::spawn(async move {
            until_hung_up(conn).await;
            error!("cargo green is gone: stopping");
            eprintln!("cargo green is gone: stopping");
            std::process::exit(1)
        }));
    }

    /// This wrapper's (lazy, single) connection to the plugin, if any.
    pub(crate) fn chitchat(&self) -> Option<&'static Client> {
        static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
        CLIENT.get_or_init(|| self.chitchat_sock.as_deref().and_then(Client::connect)).as_ref()
    }
}

/// Chitchats for as long as it lives.
pub(crate) struct Server {
    sock: String,
    mind: Arc<Mind>,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        // Dropping the listener (in the task) removes the socket file
        self.task.abort();
    }
}

impl Server {
    fn start(sock: String) -> Result<Self> {
        let Some(name) = name(&sock) else { bail!("Bad socket name {sock}") };
        let listener = ListenerOptions::new()
            .name(name)
            .try_overwrite(true)
            .create_tokio()
            .map_err(|e| anyhow!("Failed listening on {sock}: {e}"))?;
        info!("chitchatting on {sock}");
        let mind = Arc::new(Mind {
            fs: fs(),
            mds: Mutex::default(),
            built: Mutex::default(),
            stored: Mutex::default(),
            replayed: Mutex::default(),
        });
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(serve(listener, mind.clone(), stopped));
        Ok(Self { sock, mind, stop: Some(stop), task })
    }

    #[must_use]
    pub(crate) fn sock(&self) -> String {
        self.sock.clone()
    }

    /// Ends the chitchat, once wrappers are done talking (they have hung up: the build is over),
    /// and hands back what they told: what building the crates this run built cost, and what
    /// replaying the crates this run replayed cost, with what building them had (when that was
    /// recorded), and how many of the built results were stored in the results cache.
    ///
    /// Wrappers don't wait for an answer when telling us such things: this takes in the last ones.
    pub(crate) async fn shutdown(
        mut self,
    ) -> (Vec<BuildStats>, Vec<(ReplayStats, BuildStats)>, usize) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        // Not waiting forever on a wrapper that would outlive its build
        if timeout(Duration::from_secs(1), &mut self.task).await.is_err() {
            warn!("Chitchat did not end: some stats may be missing");
        }
        let built = take(&mut *self.mind.built.lock().unwrap());
        let replayed = take(&mut *self.mind.replayed.lock().unwrap());
        let stored = self.mind.stored.lock().unwrap().len();
        (built.into_values().collect(), replayed.into_values().collect(), stored)
    }
}

/// What the plugin knows, shared by all chats.
struct Mind {
    /// Spawned tasks do not see `fs()`: hold on to it.
    fs: SysFs,
    /// A plugin-owned Mds cache
    mds: Mutex<HashMap<Utf8PathBuf, Arc<Md>>>,
    /// What wrappers reported they built, for the end of build recap
    built: Mutex<HashMap<MdId, BuildStats>>,
    /// Which of those got their result stored in the results cache
    stored: Mutex<HashSet<MdId>>,
    /// What wrappers reported they replayed, with what building them had cost
    replayed: Mutex<HashMap<MdId, (ReplayStats, BuildStats)>>,
}

impl Mind {
    /// `None`: nothing to answer.
    fn answer(&self, ask: Ask) -> serde_json::Result<Option<String>> {
        match ask {
            Ask::ReadMd(path) => serde_json::to_string(&self.read_md(path).as_deref()).map(Some),
            Ask::EvictMd(path) => serde_json::to_string(&self.evict_md(&path)).map(Some),
            Ask::Built { this, stats, file, stored } => {
                self.built(this, stats, file.as_deref(), stored);
                Ok(None)
            }
            Ask::Replayed { this, replay, file } => {
                self.replayed(this, replay, &file);
                Ok(None)
            }
        }
    }

    fn read_md(&self, path: Utf8PathBuf) -> Option<Arc<Md>> {
        if let Some(md) = self.mds.lock().unwrap().get(&path) {
            return Some(md.clone());
        }
        let txt = self.fs.read_to_string(&path).inspect_err(|e| warn!("No Md {path}: {e}")).ok()?;
        let md: Md = txt.parse().inspect_err(|e| warn!("No Md {path}: {e}")).ok()?;
        let md = Arc::new(md);
        let _ = self.mds.lock().unwrap().insert(path, md.clone());
        Some(md)
    }

    fn evict_md(&self, path: &Utf8Path) {
        let _ = self.mds.lock().unwrap().remove(path);
    }

    fn built(&self, this: MdId, stats: BuildStats, file: Option<&Utf8Path>, stored: bool) {
        let _ = self.built.lock().unwrap().insert(this, stats);
        if stored {
            let _ = self.stored.lock().unwrap().insert(this);
        }
        let Some(file) = file else { return };
        let ser = toml::to_string(&stats).expect("PROOF: stats always serialize");
        if let Err(e) = self.fs.write_atomic(file, &ser) {
            warn!("Failed writing {file}: {e}");
        }
    }

    /// What building a replayed crate cost was kept by the run that built it.
    fn replayed(&self, this: MdId, replay: ReplayStats, file: &Utf8Path) {
        let built = match self.fs.read_to_string(file) {
            Ok(txt) => toml::from_str(&txt)
                .inspect_err(|e| warn!("Ignoring unreadable {file}: {e}"))
                .unwrap_or_default(),
            Err(e) => {
                warn!("Failed reading (RO) {file}: {e}");
                BuildStats::default()
            }
        };
        let _ = self.replayed.lock().unwrap().insert(this, (replay, built));
    }
}

async fn serve(listener: Listener, mind: Arc<Mind>, mut stopped: oneshot::Receiver<()>) {
    // Owned here: dropping the server hangs up on every wrapper
    let mut chats = JoinSet::new();
    loop {
        tokio::select! {
            _ = &mut stopped => break,
            accepted = listener.accept() => match accepted {
                Ok(conn) => drop(chats.spawn(chat(conn, mind.clone()))),
                Err(e) => error!("Failed accepting a chitchat: {e}"),
            },
        }
        while chats.try_join_next().is_some() {}
    }
    // Asked to stop: no more wrappers. But some may have connected, and told, without us having
    // accepted them yet.
    while let Ok(accepted) = timeout(Duration::ZERO, listener.accept()).await {
        match accepted {
            Ok(conn) => drop(chats.spawn(chat(conn, mind.clone()))),
            Err(e) => return error!("Failed accepting a chitchat: {e}"),
        }
    }
    drop(listener);
    // The wrappers left are done and about to hang up
    while chats.join_next().await.is_some() {}
}

/// Answers a wrapper, one line per ask, until it hangs up.
async fn chat(conn: AsyncStream, mind: Arc<Mind>) {
    let mut send = &conn;
    let mut lines = AsyncBufReader::new(&conn).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => return,
            Err(e) => {
                error!("Failed reading a chitchat: {e}");
                return;
            }
        };

        let mut answer = None;
        if let Err(e) = (|| -> Result<_> {
            let ask = serde_json::from_str(&line)?;
            answer = mind.answer(ask)?;
            Ok(())
        })() {
            error!("BUG: unexpected chitchat {line:?}: {e}");
            eprintln!("BUG: unexpected chitchat {line:?}: {e}");
            // A panic would only end this task (and this chat): stop the whole plugin instead.
            std::process::abort()
        }
        let Some(mut answer) = answer else { continue };
        answer.push('\n');
        if let Err(e) = send.write_all(answer.as_bytes()).await {
            return error!("Failed answering a chitchat: {e}");
        }
    }
}

/// The plugin answers asks, never anything else: this returns once it hangs up.
async fn until_hung_up(mut conn: AsyncStream) {
    let _ = conn.read(&mut [0]).await;
}

/// A wrapper's end of the chitchat.
#[derive(Debug)]
pub(crate) struct Client {
    conn: Mutex<BufReader<Stream>>,
}

impl Client {
    fn connect(sock: &str) -> Option<Self> {
        let conn = Stream::connect(name(sock)?)
            .inspect_err(|e| warn!("Not chitchatting ({sock}): {e}"))
            .ok()?;
        Some(Self { conn: Mutex::new(BufReader::new(conn)) })
    }

    /// A connection that never asks anything.
    async fn lifeline(sock: &str) -> Option<AsyncStream> {
        AsyncStream::connect(name(sock)?)
            .await
            .inspect_err(|e| warn!("No lifeline ({sock}): {e}"))
            .ok()
    }

    /// The Md at `path`, or `None`: read it off disk then.
    #[must_use]
    pub(crate) fn read_md(&self, path: &Utf8Path) -> Option<Md> {
        self.ask::<Option<Md>>(&Ask::ReadMd(path.to_owned())).flatten()
    }

    /// Have the plugin forget an Md that was just (re)written.
    pub(crate) fn evict_md(&self, path: &Utf8Path) {
        let _: Option<()> = self.ask(&Ask::EvictMd(path.to_owned()));
    }

    /// Tell the plugin what building that crate cost, which file to keep that in (if any), and
    /// whether this build is the one that stored the crate's result in the results cache.
    pub(crate) fn built(
        &self,
        this: MdId,
        stats: BuildStats,
        file: Option<&Utf8Path>,
        stored: bool,
    ) {
        let file = file.map(ToOwned::to_owned);
        self.tell(&Ask::Built { this, stats, file, stored });
    }

    /// Tell the plugin that crate was replayed at that cost, and which file says what building it
    /// cost.
    pub(crate) fn replayed(&self, this: MdId, replay: ReplayStats, file: &Utf8Path) {
        self.tell(&Ask::Replayed { this, replay, file: file.to_owned() });
    }

    /// Says something to the plugin that needs no answer.
    ///
    /// Stats are nice to have: failing to tell them is not worth stopping a build for.
    fn tell(&self, ask: &Ask) {
        let mut line = serde_json::to_string(ask).expect("PROOF: asks always serialize");
        line.push('\n');
        let mut conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        if let Err(e) = conn.get_mut().write_all(line.as_bytes()) {
            warn!("Failed telling {ask:?}: {e}");
        }
    }

    fn ask<T: DeserializeOwned>(&self, ask: &Ask) -> Option<T> {
        let mut line = serde_json::to_string(ask).expect("PROOF: asks always serialize");
        line.push('\n');
        let mut conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        if let Err(e) = conn.get_mut().write_all(line.as_bytes()) {
            panic!("BUG: failed asking {ask:?}: {e}");
        }
        line.clear();
        match conn.read_line(&mut line) {
            Ok(0) => error!("No answer to {ask:?}: plugin hung up"),
            Ok(_) => match serde_json::from_str(&line) {
                Ok(answer) => return Some(answer),
                Err(e) => panic!("BUG: unexpected answer to {ask:?}: {line:?}: {e}"),
            },
            Err(e) => panic!("BUG: failed reading answer to {ask:?}: {e}"),
        }
        drop(conn);
        None
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use camino::Utf8Path;

    use super::{Client, Server, until_hung_up};
    use crate::{
        md::{Md, MdId},
        stats::{BuildStats, ReplayStats},
        sys::{Sys, fake::FakeFs},
    };

    const MD: &str = "/w/target/debug/0123456789abcdef.toml";

    #[tokio::test(flavor = "multi_thread")]
    async fn wrappers_read_mds_until_evicted() {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        let md = Md::from(MdId::from("0123456789abcdef"));
        fs.file(MD, toml::to_string(&md).unwrap());
        fs.file("/w/Cargo.toml", "[package]\n");

        let server = Server::start(test_sock()).unwrap();
        let client = Client::connect(&server.sock()).unwrap();

        // Wrappers are blocking clients, each a process of its own
        let (client, got) = tokio::task::spawn_blocking(move || {
            let got = [MD, "/w/target/debug/0000000000000000.toml", "/w/Cargo.toml"]
                .map(|path| client.read_md(path.into()).as_ref().map(Md::this));
            (client, got)
        })
        .await
        .unwrap();
        assert_eq!(got, [Some(md.this()), None, None], "misses are the wrapper's to read");

        let rewritten = Md::from(MdId::from("fedcba9876543210"));
        fs.file(MD, toml::to_string(&rewritten).unwrap());
        let got = tokio::task::spawn_blocking(move || {
            client.evict_md(MD.into());
            client.read_md(MD.into()).as_ref().map(Md::this)
        })
        .await
        .unwrap();
        assert_eq!(got, Some(rewritten.this()), "a rewritten Md is read afresh");

        drop(server);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lifelines_end_with_the_plugin() {
        let _guard = Sys::install(Sys::fake());
        let server = Server::start(test_sock()).unwrap();
        let conn = Client::lifeline(&server.sock()).await.unwrap();

        let held = tokio::spawn(until_hung_up(conn));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!held.is_finished(), "held for as long as the plugin lives");

        drop(server);
        tokio::time::timeout(Duration::from_secs(5), held).await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn what_builds_cost_outlives_the_run() {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        let this = MdId::from("0123456789abcdef");
        let other = MdId::from("fedcba9876543210");
        let file = Utf8Path::new("/cache/stats/stage.toml");

        let run = Server::start(test_sock()).unwrap();
        let client = Client::connect(&run.sock()).unwrap();
        tokio::task::spawn_blocking(move || {
            let stats = BuildStats { remote: true, building_ms: 4_242 };
            client.built(this, stats, Some(file), true);
            client.built(other, BuildStats::default(), None, false);
        })
        .await
        .unwrap();
        let (built, replayed, stored) = run.shutdown().await;
        assert_eq!(stored, 1);
        assert_eq!(built.iter().map(|b| b.building_ms).max(), Some(4_242));
        assert_eq!(built.len(), 2);
        assert!(replayed.is_empty());

        let next = Server::start(format!("next-{}", test_sock())).unwrap();
        let client = Client::connect(&next.sock()).unwrap();
        tokio::task::spawn_blocking(move || {
            let replay = ReplayStats { replaying_ms: 40, read_bytes: 3_000 };
            client.replayed(this, replay, file);
            client.replayed(other, replay, "/cache/stats/never-written.toml".into());
        })
        .await
        .unwrap();
        let (built, mut replayed, stored) = next.shutdown().await;
        assert_eq!(stored, 0, "replays store nothing");
        assert!(built.is_empty(), "replays are not builds");
        replayed.sort_by_key(|(_, was)| was.building_ms);
        let [(none, nothing), (replay, was)] = replayed[..] else { panic!("{replayed:?}") };
        assert_eq!((replay.replaying_ms, replay.read_bytes), (40, 3_000));
        assert_eq!((was.remote, was.building_ms), (true, 4_242));
        assert_eq!((none.replaying_ms, nothing.building_ms), (40, 0), "unknown: nothing to tell");
    }

    #[test]
    fn no_server_no_client() {
        assert!(Client::connect(&test_sock()).is_none());
    }

    fn test_sock() -> String {
        let id = format!("{}-{:?}", std::process::id(), std::thread::current().id())
            .replace(|c: char| !c.is_ascii_alphanumeric(), "");
        format!("{}-test-{id}.sock", crate::PKG)
    }
}
