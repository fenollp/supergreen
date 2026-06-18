//! Minimal AWS Signature Version 4 signing for S3-compatible `PUT`s, with no extra SDK.
//!
//! Targets Cloudflare R2's S3 API endpoint (`https://<account>.r2.cloudflarestorage.com`)
//! directly — no Worker, no proxy. Reads still go through the public custom domain (see
//! [`crate::cache::result`]); only writes (publishing results) need signed requests.

use std::{collections::BTreeMap, env};

use anyhow::{Result, anyhow, bail};
use chrono::{Datelike, Timelike, Utc};
use reqwest::{
    Body, Client,
    header::{CONTENT_LENGTH, CONTENT_TYPE},
};
use ring::hmac;

/// S3-compatible credentials & target, read from `$CARGOGREEN_RESULTS_S3_*`.
pub(crate) struct S3Config {
    /// E.g. `https://<account>.r2.cloudflarestorage.com`
    endpoint: String,
    bucket: String,
    /// R2 ignores this; use `auto`.
    region: String,
    access_key: String,
    secret_key: String,
}

impl S3Config {
    /// Returns `None` when publishing isn't configured (all four required vars must be set).
    pub(crate) fn from_env() -> Option<Self> {
        let endpoint = nonempty(ENV_RESULTS_S3_ENDPOINT!())?;
        let bucket = nonempty(ENV_RESULTS_S3_BUCKET!())?;
        let access_key = nonempty(ENV_RESULTS_S3_ACCESS_KEY_ID!())?;
        let secret_key = nonempty(ENV_RESULTS_S3_SECRET_ACCESS_KEY!())?;
        let region = nonempty(ENV_RESULTS_S3_REGION!()).unwrap_or_else(|| "auto".to_owned());
        Some(Self { endpoint, bucket, region, access_key, secret_key })
    }

    fn host(&self) -> &str {
        self.endpoint
            .strip_prefix("https://")
            .or_else(|| self.endpoint.strip_prefix("http://"))
            .unwrap_or(&self.endpoint)
            .trim_end_matches('/')
    }
}

fn nonempty(var: &str) -> Option<String> {
    env::var(var).ok().filter(|s| !s.is_empty())
}

/// `PUT` `body` (whose hex SHA-256 is `payload_sha256`, length `content_len`) to
/// `s3://{bucket}/{key}`, signed with SigV4. `extra` headers (e.g. `x-amz-meta-*`) are signed
/// and sent. Fails on any non-2xx response.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn put_object(
    cfg: &S3Config,
    client: &Client,
    key: &str,
    body: Body,
    content_len: u64,
    payload_sha256: &str,
    content_type: &str,
    extra: &BTreeMap<String, String>,
) -> Result<()> {
    let host = cfg.host().to_owned();
    let canonical_uri = format!("/{}/{}", uri_encode(&cfg.bucket), uri_encode(key));
    let url = format!("{}{canonical_uri}", cfg.endpoint.trim_end_matches('/'));

    let now = Utc::now();
    let amz_date = format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    let date_stamp = format!("{:04}{:02}{:02}", now.year(), now.month(), now.day());

    // Headers to sign, canonicalised: lowercase keys, sorted (BTreeMap), trimmed values.
    let mut signed: BTreeMap<String, String> = BTreeMap::new();
    signed.insert("host".to_owned(), host);
    signed.insert("x-amz-content-sha256".to_owned(), payload_sha256.to_owned());
    signed.insert("x-amz-date".to_owned(), amz_date.clone());
    for (k, v) in extra {
        signed.insert(k.to_lowercase(), v.clone());
    }

    let signed_headers = signed.keys().cloned().collect::<Vec<_>>().join(";");
    let canonical_headers =
        signed.iter().map(|(k, v)| format!("{k}:{}\n", v.trim())).collect::<String>();

    let canonical_request =
        format!("PUT\n{canonical_uri}\n\n{canonical_headers}\n{signed_headers}\n{payload_sha256}");
    let scope = format!("{date_stamp}/{}/s3/aws4_request", cfg.region);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256::digest(canonical_request.as_bytes())
    );

    let signing_key = signing_key(&cfg.secret_key, &date_stamp, &cfg.region, "s3");
    let signature = hex(hmac_sha256(signing_key.as_ref(), string_to_sign.as_bytes()).as_ref());

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        cfg.access_key
    );

    let mut req = client
        .put(&url)
        .header("x-amz-date", &amz_date)
        .header("x-amz-content-sha256", payload_sha256)
        .header("authorization", authorization)
        .header(CONTENT_TYPE, content_type)
        .header(CONTENT_LENGTH, content_len);
    for (k, v) in extra {
        req = req.header(k.as_str(), v.as_str());
    }

    let resp = req.body(body).send().await.map_err(|e| anyhow!("PUT {url} failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("PUT {url} -> {status}: {body}");
    }
    Ok(())
}

fn hmac_sha256(key: &[u8], msg: &[u8]) -> hmac::Tag {
    hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), msg)
}

/// Derive the SigV4 signing key: HMAC chain over date, region, service, "aws4_request".
fn signing_key(secret: &str, date_stamp: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), date_stamp.as_bytes());
    let k_region = hmac_sha256(k_date.as_ref(), region.as_bytes());
    let k_service = hmac_sha256(k_region.as_ref(), service.as_bytes());
    hmac_sha256(k_service.as_ref(), b"aws4_request").as_ref().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// AWS-style percent-encoding for a single path segment (no `/` expected in a key segment).
/// Unreserved set per RFC 3986: `A-Z a-z 0-9 - _ . ~`.
fn uri_encode(s: &str) -> String {
    use std::fmt::Write;
    s.bytes().fold(String::with_capacity(s.len()), |mut out, b| {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_key_matches_aws_reference() {
        // AWS SigV4 reference vector (eu-central-1 / s3 / 20150830).
        // https://docs.aws.amazon.com/general/latest/gr/signature-v4-examples.html
        let key =
            signing_key("wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", "20150830", "us-east-1", "iam");
        assert_eq!(hex(&key), "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9");
    }

    #[test]
    fn uri_encode_keeps_unreserved() {
        assert_eq!(uri_encode("out-19ffbea695cb4980.tar.gz"), "out-19ffbea695cb4980.tar.gz");
        assert_eq!(uri_encode("a b+c"), "a%20b%2Bc");
    }

    #[test]
    fn hex_pads() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff]), "000fff");
    }
}
