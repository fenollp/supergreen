//! A remote store of build results, shared between machines.
//!
//! Reads are plain `GET`s against a (public, read-only) base URL, e.g. an R2 bucket's custom domain.
//! Writes are `PUT`s against an S3-compatible API, signed with AWS Signature Version 4: no SDK,
//! no proxy or Worker in between.
//!
//! Each `{stage}.tar.zst` goes along with a `{stage}.tar.zst.sha256` sidecar, written last.
//! A download is only kept when it matches its sidecar.

use std::collections::BTreeMap;

use anyhow::{Result, anyhow, bail};
use camino::Utf8Path;
use chrono::{DateTime, Datelike, Timelike, Utc};
use log::{debug, info, warn};
use ring::hmac;
use tokio::fs;
use uuid::Uuid;

use crate::{
    all_our_envs::{
        CARGOGREEN_RESULTS_S3_ACCESS_KEY_ID, CARGOGREEN_RESULTS_S3_BUCKET,
        CARGOGREEN_RESULTS_S3_ENDPOINT, CARGOGREEN_RESULTS_S3_SECRET_ACCESS_KEY,
    },
    dirs::Dirs,
    redact::Vars,
    stage::Stage,
    sys::http,
};

/// What to fetch from and publish to. Both off by default.
#[derive(Debug, Default)]
pub(crate) struct Remote {
    /// Where to `GET` results from
    base_url: Option<String>,

    /// Where to `PUT` results to
    s3: Option<S3>,
}

/// An S3-compatible API endpoint, with credentials.
struct S3 {
    /// E.g. `https://<account-id>.r2.cloudflarestorage.com`
    endpoint: String,
    bucket: String,
    region: String,
    access_key_id: String,
    secret_access_key: String,
}

impl std::fmt::Debug for S3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { endpoint, bucket, region, .. } = self;
        write!(f, "S3 {{ endpoint: {endpoint:?}, bucket: {bucket:?}, region: {region:?}, .. }}")
    }
}

impl Remote {
    /// Reads `$CARGOGREEN_RESULTS_*`. Nothing remote happens when `offline`.
    pub(crate) fn from_env(env: &Vars, offline: bool) -> Result<Self> {
        let get = |var: &str| env.get(var).map(String::as_str).filter(|val| !val.is_empty());
        let url = |var: &str| -> Result<Option<String>> {
            let Some(val) = get(var) else { return Ok(None) };
            if !val.starts_with("https://") && !val.starts_with("http://") {
                bail!("${var}={val:?} is not an http(s) URL")
            }
            Ok(Some(val.trim_end_matches('/').to_owned()))
        };

        let base_url = url(CARGOGREEN_RESULTS_BASE_URL!())?;

        let endpoint = url(CARGOGREEN_RESULTS_S3_ENDPOINT!())?;
        let bucket = get(CARGOGREEN_RESULTS_S3_BUCKET!());
        let access_key_id = get(CARGOGREEN_RESULTS_S3_ACCESS_KEY_ID!());
        let secret_access_key = get(CARGOGREEN_RESULTS_S3_SECRET_ACCESS_KEY!());
        let s3 = match (endpoint, bucket, access_key_id, secret_access_key) {
            (None, None, None, None) => None,
            (Some(endpoint), Some(bucket), Some(access_key_id), Some(secret_access_key)) => {
                Some(S3 {
                    endpoint,
                    bucket: bucket.to_owned(),
                    region: get(CARGOGREEN_RESULTS_S3_REGION!()).unwrap_or("auto").to_owned(),
                    access_key_id: access_key_id.to_owned(),
                    secret_access_key: secret_access_key.to_owned(),
                })
            }
            (endpoint, bucket, access_key_id, secret_access_key) => {
                let missing: Vec<_> = [
                    (endpoint.is_none(), CARGOGREEN_RESULTS_S3_ENDPOINT),
                    (bucket.is_none(), CARGOGREEN_RESULTS_S3_BUCKET),
                    (access_key_id.is_none(), CARGOGREEN_RESULTS_S3_ACCESS_KEY_ID),
                    (secret_access_key.is_none(), CARGOGREEN_RESULTS_S3_SECRET_ACCESS_KEY),
                ]
                .into_iter()
                .filter_map(|(missing, var)| missing.then_some(var))
                .collect();
                bail!("Publishing results also requires {}", missing.join(", "))
            }
        };

        if offline {
            return Ok(Self::default());
        }
        Ok(Self { base_url, s3 })
    }

    /// On a local miss: tries downloading `target`'s result into `dirs`.
    ///
    /// Returns whether it is now there. Remote failures are misses: only local ones bubble up.
    pub(crate) async fn fetch(&self, dirs: &Dirs, target: &Stage) -> Result<bool> {
        let Some(ref base_url) = self.base_url else { return Ok(false) };
        let dst = dirs.result_from_stage(target);
        let name = dst.file_name().expect("PROOF: results have a name");
        let url = format!("{base_url}/{name}");

        let Some(zst) = get(&url).await else { return Ok(false) };
        // Not having a checksum may mean the result is being published right now
        let Some(sha) = get(&format!("{url}.sha256")).await else { return Ok(false) };

        let expected = String::from_utf8_lossy(&sha);
        let actual = sha256::digest(zst.as_slice());
        if actual != expected.trim() {
            warn!("discarding {url}: checksum is {actual}, not {expected}");
            return Ok(false);
        }
        info!("fetched {}B from {url}", zst.len());

        write_atomic(&dirs.tmp, &dst, &zst).await?;
        Ok(true)
    }

    /// Uploads a fresh result. Best effort: failures only get logged.
    pub(crate) async fn publish(&self, target: &Stage, zst: &[u8]) {
        let Some(ref s3) = self.s3 else { return };
        if let Err(e) = s3.publish(target, zst, Utc::now()).await {
            warn!("failed publishing {target}'s result: {e}");
        }
    }
}

async fn get(url: &str) -> Option<Vec<u8>> {
    debug!("GET {url}");
    match http().get(url).await {
        Ok(Some(body)) => Some(body),
        Ok(None) => {
            debug!("no {url}");
            None
        }
        Err(e) => {
            warn!("{e}");
            None
        }
    }
}

/// Writes then renames, so readers never see a partial file.
async fn write_atomic(tmp: &Utf8Path, dst: &Utf8Path, data: &[u8]) -> Result<()> {
    let tmp = tmp.join(format!("{}.tar.zst", Uuid::new_v4()));
    fs::write(&tmp, data).await.map_err(|e| anyhow!("Failed writing {tmp}: {e}"))?;
    // NOTE: TOCTOU on dst is okay as long as `mv` is atomic
    fs::rename(&tmp, dst).await.map_err(|e| anyhow!("Failed `mv {tmp} {dst}`: {e}"))
}

impl S3 {
    async fn publish(&self, target: &Stage, zst: &[u8], now: DateTime<Utc>) -> Result<()> {
        let key = format!("{target}.tar.zst");
        let sha = sha256::digest(zst);

        info!("PUTing {}B to {key}", zst.len());
        let (url, headers) = self.signed_put(&key, &sha, "application/zstd", now);
        http().put(&url, &headers, zst.to_vec()).await?;

        // Last, so readers never get a result whose checksum is missing
        let (url, headers) =
            self.signed_put(&format!("{key}.sha256"), &sha256::digest(&sha), "text/plain", now);
        http().put(&url, &headers, sha.clone().into_bytes()).await?;

        info!("published {key} ({sha})");
        Ok(())
    }

    /// URL and headers of a path-style `PUT` of a body whose hex SHA-256 is `payload_sha256`.
    #[must_use]
    fn signed_put(
        &self,
        key: &str,
        payload_sha256: &str,
        content_type: &str,
        now: DateTime<Utc>,
    ) -> (String, Vec<(String, String)>) {
        let host = self.endpoint.split_once("://").map_or(&*self.endpoint, |(_, host)| host);
        let path = format!("/{}/{}", uri_encode(&self.bucket), uri_encode(key));

        let mut headers = BTreeMap::new();
        headers.insert("content-type".to_owned(), content_type.to_owned());
        headers.insert("host".to_owned(), host.to_owned());
        headers.insert("x-amz-content-sha256".to_owned(), payload_sha256.to_owned());
        headers.insert("x-amz-date".to_owned(), amz_date(now));

        let authorization = self.authorization("PUT", &path, &headers, payload_sha256, now);
        // reqwest sets Host itself
        headers.remove("host");
        headers.insert("authorization".to_owned(), authorization);

        (format!("{}{path}", self.endpoint), headers.into_iter().collect())
    }

    /// <https://docs.aws.amazon.com/AmazonS3/latest/API/sig-v4-header-based-auth.html>
    ///
    /// `headers`' keys are lowercase, and include `host` and `x-amz-date`. No query string.
    #[must_use]
    fn authorization(
        &self,
        method: &str,
        path: &str,
        headers: &BTreeMap<String, String>,
        payload_sha256: &str,
        now: DateTime<Utc>,
    ) -> String {
        let Self { region, access_key_id, secret_access_key, .. } = self;

        let signed_headers = headers.keys().cloned().collect::<Vec<_>>().join(";");
        let canonical_headers: String =
            headers.iter().map(|(k, v)| format!("{k}:{}\n", v.trim())).collect();
        let canonical_request =
            format!("{method}\n{path}\n\n{canonical_headers}\n{signed_headers}\n{payload_sha256}");

        let date = date_stamp(now);
        let scope = format!("{date}/{region}/s3/aws4_request");
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{scope}\n{}",
            amz_date(now),
            sha256::digest(&canonical_request)
        );

        let key = signing_key(secret_access_key, &date, region, "s3");
        let signature = hex(hmac_sha256(&key, string_to_sign.as_bytes()).as_ref());

        format!(
            "AWS4-HMAC-SHA256 Credential={access_key_id}/{scope}, SignedHeaders={signed_headers}, Signature={signature}"
        )
    }
}

#[must_use]
fn amz_date(now: DateTime<Utc>) -> String {
    let (h, m, s) = (now.hour(), now.minute(), now.second());
    format!("{}T{h:02}{m:02}{s:02}Z", date_stamp(now))
}

#[must_use]
fn date_stamp(now: DateTime<Utc>) -> String {
    format!("{:04}{:02}{:02}", now.year(), now.month(), now.day())
}

#[must_use]
fn hmac_sha256(key: &[u8], msg: &[u8]) -> hmac::Tag {
    hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), msg)
}

/// HMAC chain over date, region, service then "aws4_request"
#[must_use]
fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(k_date.as_ref(), region.as_bytes());
    let k_service = hmac_sha256(k_region.as_ref(), service.as_bytes());
    hmac_sha256(k_service.as_ref(), b"aws4_request").as_ref().to_vec()
}

#[must_use]
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Percent-encodes all but RFC 3986's unreserved characters (`/` included: keys have none)
#[must_use]
fn uri_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use camino::Utf8PathBuf;
    use chrono::TimeZone;

    use super::*;
    use crate::sys::{Sys, fake::FakeHttp};

    const BASE: &str = "https://results.example.com";
    const ENDPOINT: &str = "https://acc.r2.cloudflarestorage.com";

    fn env(kvs: &[(&str, &str)]) -> Vars {
        kvs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    fn s3_env() -> Vec<(&'static str, &'static str)> {
        vec![
            (CARGOGREEN_RESULTS_S3_ENDPOINT!(), ENDPOINT),
            (CARGOGREEN_RESULTS_S3_BUCKET!(), "bucket"),
            (CARGOGREEN_RESULTS_S3_ACCESS_KEY_ID!(), "AKID"),
            (CARGOGREEN_RESULTS_S3_SECRET_ACCESS_KEY!(), "SECRET"),
        ]
    }

    fn target() -> Stage {
        Stage::output(0x5555555555555555_u64.into()).unwrap()
    }

    /// Runs `f` with a fake HTTP and a scratch directory standing in for results & tmp dirs
    fn with(f: impl AsyncFnOnce(Arc<FakeHttp>, Dirs)) {
        let fake = FakeHttp::new();
        let _guard = Sys::install(Sys { http: fake.clone(), ..Sys::fake() });

        let dir: Utf8PathBuf =
            std::env::temp_dir().join(Uuid::new_v4().to_string()).try_into().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let dirs = Dirs { tmp: dir.clone(), results: dir.clone(), buildkit: dir.clone() };

        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f(fake, dirs));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn off_unless_configured() {
        let remote = Remote::from_env(&env(&[]), false).unwrap();
        assert!(remote.base_url.is_none());
        assert!(remote.s3.is_none());
    }

    #[test]
    fn off_when_offline() {
        let mut kvs = s3_env();
        kvs.push((CARGOGREEN_RESULTS_BASE_URL!(), BASE));
        let remote = Remote::from_env(&env(&kvs), true).unwrap();
        assert!(remote.base_url.is_none());
        assert!(remote.s3.is_none());
    }

    #[test]
    fn urls_are_checked_and_trimmed() {
        let remote =
            Remote::from_env(&env(&[(CARGOGREEN_RESULTS_BASE_URL!(), "https://a.b/c/")]), false);
        assert_eq!(remote.unwrap().base_url.as_deref(), Some("https://a.b/c"));

        let err = Remote::from_env(&env(&[(CARGOGREEN_RESULTS_BASE_URL!(), "a.b")]), false);
        assert!(err.unwrap_err().to_string().contains("is not an http(s) URL"));
    }

    #[test]
    fn publishing_requires_all_settings() {
        let remote = Remote::from_env(&env(&s3_env()), false).unwrap();
        assert_eq!(remote.s3.unwrap().region, "auto");

        let partial = &s3_env()[..2];
        let err = Remote::from_env(&env(partial), false).unwrap_err().to_string();
        assert_eq!(
            err,
            "Publishing results also requires $CARGOGREEN_RESULTS_S3_ACCESS_KEY_ID, $CARGOGREEN_RESULTS_S3_SECRET_ACCESS_KEY"
        );
    }

    #[test]
    fn secrets_stay_out_of_debug() {
        let remote = Remote::from_env(&env(&s3_env()), false).unwrap();
        let shown = format!("{remote:?}");
        assert!(!shown.contains("SECRET"), "{shown}");
        assert!(!shown.contains("AKID"), "{shown}");
    }

    /// <https://docs.aws.amazon.com/AmazonS3/latest/API/sig-v4-header-based-auth.html>
    /// (example: GET Object)
    #[test]
    fn signature_matches_aws_example() {
        let s3 = S3 {
            endpoint: "https://examplebucket.s3.amazonaws.com".to_owned(),
            bucket: "examplebucket".to_owned(),
            region: "us-east-1".to_owned(),
            access_key_id: "AKIAIOSFODNN7EXAMPLE".to_owned(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_owned(),
        };
        let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let now = Utc.with_ymd_and_hms(2013, 5, 24, 0, 0, 0).unwrap();
        let headers = [
            ("host", "examplebucket.s3.amazonaws.com"),
            ("range", "bytes=0-9"),
            ("x-amz-content-sha256", empty),
            ("x-amz-date", "20130524T000000Z"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();

        assert_eq!(
            s3.authorization("GET", "/test.txt", &headers, empty, now),
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn signing_key_matches_aws_example() {
        // https://docs.aws.amazon.com/general/latest/gr/signature-v4-examples.html
        let key =
            signing_key("wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", "20150830", "us-east-1", "iam");
        assert_eq!(hex(&key), "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9");
    }

    #[test]
    fn uri_encoding() {
        assert_eq!(uri_encode("out-19ffbea695cb4980.tar.zst"), "out-19ffbea695cb4980.tar.zst");
        assert_eq!(uri_encode("a b+c/d"), "a%20b%2Bc%2Fd");
    }

    #[test]
    fn fetching_a_checked_result() {
        with(async |fake, dirs| {
            let url = format!("{BASE}/{}.tar.zst", target());
            fake.serve(&url, "zst");
            fake.serve(&format!("{url}.sha256"), format!("{}\n", sha256::digest("zst")));

            let remote = Remote { base_url: Some(BASE.to_owned()), s3: None };
            assert!(remote.fetch(&dirs, &target()).await.unwrap());
            assert_eq!(std::fs::read(dirs.result_from_stage(&target())).unwrap(), b"zst");
        });
    }

    #[test]
    fn fetching_misses() {
        let checksum = |fake: &FakeHttp, url: &str, sum: &str| {
            fake.serve(url, "zst");
            fake.serve(&format!("{url}.sha256"), sum.to_owned());
        };
        for (case, setup) in [
            ("not found", Box::new(|_: &FakeHttp, _: &str| {}) as Box<dyn Fn(&FakeHttp, &str)>),
            ("network", Box::new(|fake: &FakeHttp, url: &str| fake.fail(url))),
            ("no checksum", Box::new(|fake: &FakeHttp, url: &str| fake.serve(url, "zst"))),
            ("bad checksum", Box::new(move |fake: &FakeHttp, url: &str| checksum(fake, url, "0"))),
        ] {
            with(async |fake, dirs| {
                setup(&fake, &format!("{BASE}/{}.tar.zst", target()));
                let remote = Remote { base_url: Some(BASE.to_owned()), s3: None };
                assert!(!remote.fetch(&dirs, &target()).await.unwrap(), "{case}");
                assert!(!dirs.result_from_stage(&target()).exists(), "{case}");
            });
        }
    }

    #[test]
    fn fetching_nothing_when_off() {
        with(async |fake, dirs| {
            assert!(!Remote::default().fetch(&dirs, &target()).await.unwrap());
            assert_eq!(fake.requests(), vec![]);
        });
    }

    #[test]
    fn publishing_puts_the_result_then_its_checksum() {
        with(async |fake, _dirs| {
            let Remote { s3: Some(s3), .. } = Remote::from_env(&env(&s3_env()), false).unwrap()
            else {
                unreachable!()
            };
            let now = Utc.with_ymd_and_hms(2026, 10, 19, 12, 34, 56).unwrap();
            s3.publish(&target(), b"zst", now).await.unwrap();

            let reqs = fake.requests();
            let urls: Vec<_> = reqs.iter().map(|req| (req.method, req.url.as_str())).collect();
            let key = format!("{ENDPOINT}/bucket/{}.tar.zst", target());
            assert_eq!(urls, [("PUT", key.as_str()), ("PUT", &format!("{key}.sha256"))]);

            assert_eq!(reqs[0].body, b"zst");
            assert_eq!(reqs[1].body, sha256::digest("zst").as_bytes());

            let header = |name: &str| {
                reqs[0].headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
            };
            assert_eq!(header("x-amz-content-sha256"), Some(sha256::digest("zst").as_str()));
            assert_eq!(header("x-amz-date"), Some("20261019T123456Z"));
            assert_eq!(header("host"), None);
            let auth = header("authorization").unwrap();
            assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKID/20261019/auto/s3/aws4_request, SignedHeaders=content-type;host;x-amz-content-sha256;x-amz-date, Signature="), "{auth}");
        });
    }

    #[test]
    fn publishing_failures_are_not_fatal() {
        with(async |fake, _dirs| {
            let remote = Remote::from_env(&env(&s3_env()), false).unwrap();
            fake.fail(&format!("{ENDPOINT}/bucket/{}.tar.zst", target()));
            remote.publish(&target(), b"zst").await;
            assert_eq!(fake.requests().len(), 1, "checksum must not go without its result");
        });
    }
}
