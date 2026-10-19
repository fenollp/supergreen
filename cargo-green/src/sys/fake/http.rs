use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
};

use anyhow::{Result, anyhow};
use futures::future::LocalBoxFuture;

use crate::sys::Http;

/// Canned bodies per URL (some failing), plus a log of requests.
pub(crate) struct FakeHttp {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    bodies: HashMap<String, Vec<u8>>,
    failing: Vec<String>,
    requests: Vec<Request>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Request {
    pub(crate) method: &'static str,
    pub(crate) url: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
}

impl FakeHttp {
    #[must_use]
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self { inner: Mutex::default() })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }

    /// Serves `body` at `url`
    pub(crate) fn serve(&self, url: &str, body: impl Into<Vec<u8>>) {
        self.lock().bodies.insert(url.to_owned(), body.into());
    }

    /// Requests to `url` fail as if the network did
    pub(crate) fn fail(&self, url: &str) {
        self.lock().failing.push(url.to_owned());
    }

    #[must_use]
    pub(crate) fn requests(&self) -> Vec<Request> {
        self.lock().requests.clone()
    }
}

impl Http for FakeHttp {
    fn get<'a>(&'a self, url: &'a str) -> LocalBoxFuture<'a, Result<Option<Vec<u8>>>> {
        Box::pin(async move {
            let mut inner = self.lock();
            let req = Request { method: "GET", url: url.to_owned(), headers: vec![], body: vec![] };
            inner.requests.push(req);
            if inner.failing.iter().any(|failing| failing == url) {
                return Err(anyhow!("GET {url}: fake network failure"));
            }
            Ok(inner.bodies.get(url).cloned())
        })
    }

    fn put<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(String, String)],
        body: Vec<u8>,
    ) -> LocalBoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut inner = self.lock();
            let req =
                Request { method: "PUT", url: url.to_owned(), headers: headers.to_vec(), body };
            inner.requests.push(req.clone());
            if inner.failing.iter().any(|failing| failing == url) {
                return Err(anyhow!("PUT {url}: fake network failure"));
            }
            inner.bodies.insert(req.url, req.body);
            Ok(())
        })
    }
}
