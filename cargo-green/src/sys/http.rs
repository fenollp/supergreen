use anyhow::Result;
use futures::future::LocalBoxFuture;

/// HTTP operations, against a remote results store.
pub(crate) trait Http: Send + Sync {
    /// GETs a body. `Ok(None)` when not found.
    fn get<'a>(&'a self, url: &'a str) -> LocalBoxFuture<'a, Result<Option<Vec<u8>>>>;

    /// PUTs a body, along with headers. Fails on non-2xx responses.
    fn put<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(String, String)],
        body: Vec<u8>,
    ) -> LocalBoxFuture<'a, Result<()>>;
}
