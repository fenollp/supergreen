//! <cargo green plugin> <--> many <cargo green rustc wrappers>
//!
//! Assumption is server may not always run nor be reliable: wrappers then do without.
//!
//! * serves Md.s from plugin memory and wrappers fallback to disk
//!
//! A wrapper writes an [`Ask`] as a JSON line, the plugin answers with a JSON line
//! whose type depends on the ask.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

use anyhow::{Result, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerOptions, Name, Stream, ToFsName, ToNsName,
    tokio::{Listener, Stream as AsyncStream, prelude::*},
    traits::Stream as _,
};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader},
    task::JoinHandle,
};

use crate::{
    dirs::Paths,
    md::Md,
    sys::{SysFs, fs},
};

/// What a wrapper asks the plugin.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Ask {
    /// Answered with an `Option<Md>`: on `None` the wrapper reads that Md off disk itself.
    ReadMd(Utf8PathBuf),
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

    /// This wrapper's (lazy, single) connection to the plugin, if any.
    pub(crate) fn chitchat(&self) -> Option<&'static Client> {
        static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
        CLIENT.get_or_init(|| self.chitchat_sock.as_deref().and_then(Client::connect)).as_ref()
    }
}

/// Chitchats for as long as it lives.
pub(crate) struct Server {
    sock: String,
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
        let name = name(&sock).ok_or_else(|| anyhow!("Bad socket name {sock}"))?;
        let listener = ListenerOptions::new()
            .name(name)
            .try_overwrite(true)
            .create_tokio()
            .map_err(|e| anyhow!("Failed listening on {sock}: {e}"))?;
        info!("chitchatting on {sock}");
        let mind = Arc::new(Mind { fs: fs(), mds: Mutex::default() });
        let task = tokio::spawn(serve(listener, mind));
        Ok(Self { sock, task })
    }

    #[must_use]
    pub(crate) fn sock(&self) -> String {
        self.sock.clone()
    }
}

/// What the plugin knows, shared by all chats.
struct Mind {
    /// Spawned tasks do not see `fs()`: hold on to it.
    fs: SysFs,
    mds: Mutex<HashMap<Utf8PathBuf, Arc<Md>>>,
}

impl Mind {
    fn answer(&self, ask: Ask) -> serde_json::Result<String> {
        match ask {
            Ask::ReadMd(path) => serde_json::to_string(&self.read_md(path).as_deref()),
        }
    }

    fn read_md(&self, path: Utf8PathBuf) -> Option<Arc<Md>> {
        if let Some(md) = self.mds.lock().unwrap().get(&path) {
            return Some(md.clone());
        }
        // Two wrappers may race to read the same Md: both get the same answer.
        // Misses are not kept: the Md may show up later.
        let txt =
            self.fs.read_to_string(&path).inspect_err(|e| debug!("No Md {path}: {e}")).ok()?;
        let md: Md = txt.parse().inspect_err(|e| debug!("No Md {path}: {e}")).ok()?;
        let md = Arc::new(md);
        let _ = self.mds.lock().unwrap().insert(path, md.clone());
        Some(md)
    }
}

async fn serve(listener: Listener, mind: Arc<Mind>) {
    loop {
        match listener.accept().await {
            Ok(conn) => drop(tokio::spawn(chat(conn, mind.clone()))),
            Err(e) => error!("Failed accepting a chitchat: {e}"),
        }
    }
}

/// Answers a wrapper, one line per ask, until it hangs up.
async fn chat(conn: AsyncStream, mind: Arc<Mind>) {
    let mut send = &conn;
    let mut lines = AsyncBufReader::new(&conn).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => return,
            Err(e) => return error!("Failed reading a chitchat: {e}"),
        };
        // Plugin and wrappers are the same executable: they never disagree
        let ask = match serde_json::from_str(&line) {
            Ok(ask) => ask,
            Err(e) => return error!("BUG: unexpected chitchat {line:?}: {e}"),
        };
        let mut answer = match mind.answer(ask) {
            Ok(answer) => answer,
            Err(e) => return error!("BUG: failed answering {line:?}: {e}"),
        };
        answer.push('\n');
        if let Err(e) = send.write_all(answer.as_bytes()).await {
            return error!("Failed answering a chitchat: {e}");
        }
    }
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

    /// The Md at `path`, or `None`: read it off disk then.
    #[must_use]
    pub(crate) fn read_md(&self, path: &Utf8Path) -> Option<Md> {
        self.ask::<Option<Md>>(&Ask::ReadMd(path.to_owned())).flatten()
    }

    fn ask<T: DeserializeOwned>(&self, ask: &Ask) -> Option<T> {
        let mut line = serde_json::to_string(ask).expect("PROOF: asks always serialize");
        line.push('\n');
        let mut conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        if let Err(e) = conn.get_mut().write_all(line.as_bytes()) {
            error!("Failed asking {ask:?}: {e}");
            return None;
        }
        line.clear();
        match conn.read_line(&mut line) {
            Ok(0) => error!("No answer to {ask:?}: plugin hung up"),
            Ok(_) => match serde_json::from_str(&line) {
                Ok(answer) => return Some(answer),
                Err(e) => error!("BUG: unexpected answer to {ask:?}: {line:?}: {e}"),
            },
            Err(e) => error!("Failed reading answer to {ask:?}: {e}"),
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        md::MdId,
        sys::{Sys, fake::FakeFs},
    };

    const MD: &str = "/w/target/debug/0123456789abcdef.toml";

    #[tokio::test(flavor = "multi_thread")]
    async fn wrappers_read_mds() {
        let fakefs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fakefs.clone(), ..Sys::fake() });
        let md = Md::from(MdId::from("0123456789abcdef"));
        fakefs.file(MD, toml::to_string(&md).unwrap());
        fakefs.file("/w/Cargo.toml", "[package]\n");

        let server = Server::start(test_sock()).unwrap();
        let client = Client::connect(&server.sock()).unwrap();

        // Wrappers are blocking clients, each a process of its own
        let got = tokio::task::spawn_blocking(move || {
            [MD, "/w/target/debug/0000000000000000.toml", "/w/Cargo.toml"]
                .map(|path| client.read_md(path.into()).as_ref().map(Md::this))
        })
        .await
        .unwrap();
        assert_eq!(got, [Some(md.this()), None, None], "misses are the wrapper's to read");

        drop(server);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_plugin_hangs_up_on_nonsense() {
        let _guard = Sys::install(Sys::fake());
        let server = Server::start(test_sock()).unwrap();
        let client = Client::connect(&server.sock()).unwrap();

        let got = tokio::task::spawn_blocking(move || {
            let mut line = String::new();
            let read = {
                let mut conn = client.conn.lock().unwrap();
                conn.get_mut().write_all(b"nonsense\n").unwrap();
                conn.read_line(&mut line).unwrap()
            };
            (read, client.read_md(MD.into()).is_none())
        })
        .await
        .unwrap();
        assert_eq!(got, (0, true));

        drop(server);
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
