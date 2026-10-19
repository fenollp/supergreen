use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use futures::future::LocalBoxFuture;
use reqwest::{Client, StatusCode};

use crate::sys::Http;

pub(crate) struct RealHttp;

fn client() -> Result<Client> {
    Client::builder()
        .connect_timeout(Duration::from_secs(4))
        .build()
        .map_err(|e| anyhow!("HTTP client's config/TLS failed: {e}"))
}

impl Http for RealHttp {
    fn get<'a>(&'a self, url: &'a str) -> LocalBoxFuture<'a, Result<Option<Vec<u8>>>> {
        Box::pin(async move {
            let resp = client()?.get(url).send().await.map_err(|e| anyhow!("GET {url}: {e}"))?;
            if resp.status() == StatusCode::NOT_FOUND {
                return Ok(None);
            }
            let resp = resp.error_for_status().map_err(|e| anyhow!("GET {url}: {e}"))?;
            let body = resp.bytes().await.map_err(|e| anyhow!("GET {url} (body): {e}"))?;
            Ok(Some(body.to_vec()))
        })
    }

    fn put<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(String, String)],
        body: Vec<u8>,
    ) -> LocalBoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut req = client()?.put(url);
            for (k, v) in headers {
                req = req.header(k, v);
            }
            let resp = req.body(body).send().await.map_err(|e| anyhow!("PUT {url}: {e}"))?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                bail!("PUT {url}: {status} {body}")
            }
            Ok(())
        })
    }
}
