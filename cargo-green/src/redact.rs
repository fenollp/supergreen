use std::{collections::BTreeMap, fmt, ops::Deref};

use serde::{Deserialize, Serialize};

use crate::{all_our_envs::PREFIX, runner::Runner, wrap::pass_env};

/// A snapshot of the process environment, read once in `main`.
///
/// Its `Debug` output redacts values, see [`crate::redact`].
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Vars(BTreeMap<String, String>);

impl fmt::Debug for Vars {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter().map(|(var, val)| (var, redact(var, val)))).finish()
    }
}

impl Deref for Vars {
    type Target = BTreeMap<String, String>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FromIterator<(String, String)> for Vars {
    fn from_iter<I: IntoIterator<Item = (String, String)>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl<const N: usize> From<[(String, String); N]> for Vars {
    fn from(kvs: [(String, String); N]) -> Self {
        Self(kvs.into())
    }
}

impl<'a> IntoIterator for &'a Vars {
    type Item = (&'a String, &'a String);
    type IntoIter = std::collections::btree_map::Iter<'a, String, String>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[test]
fn vars_debug_redacts() {
    let env: Vars = [("GH_TOKEN", "0123456789"), ("CARGO_PKG_NAME", "green")]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    assert_eq!(format!("{env:?}"), r#"{"CARGO_PKG_NAME": "green", "GH_TOKEN": "01234***"}"#);
}

/// Displays as `VAR=valu**` unless allowed
pub(crate) struct Redacted<'a>(pub(crate) &'a str, pub(crate) &'a str);

impl fmt::Debug for Redacted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self(var, val) = self;
        write!(f, "{var}={}", redact(var, val))
    }
}

#[must_use]
fn redact(var: &str, val: &str) -> String {
    if !allowed(var) {
        return partial(val);
    }
    if var.to_lowercase().ends_with("_proxy") {
        return hide_password(val);
    }
    val.to_owned()
}

fn allowed(var: &str) -> bool {
    if looks_secret(var) {
        return false;
    }
    let (pass, skip, only_buildrs) = pass_env(var);
    (pass && !skip)
        || only_buildrs
        || var.starts_with(PREFIX)
        || Runner::is_env(var)
        || [
            "HOME",
            "LANG",
            "PWD",
            "RUST_BACKTRACE",
            "RUST_LOG",
            "RUST_MIN_STACK",
            "RUSTUP_HOME",
            "RUSTUP_TOOLCHAIN",
            "SHELL",
            "TMPDIR",
            "USER",
            "XDG_RUNTIME_DIR",
        ]
        .contains(&var)
}

fn looks_secret(var: &str) -> bool {
    let var = var.to_uppercase();
    ["AUTH", "COOKIE", "CREDENTIAL", "KEY", "PASSPHRASE", "PASSW", "PRIVATE", "SECRET", "TOKEN"]
        .iter()
        .any(|word| var.contains(word))
}

fn partial(val: &str) -> String {
    if val.is_empty() {
        return String::new();
    }
    let shown = (val.chars().count() / 2).min(5);
    format!("{}***", val.chars().take(shown).collect::<String>())
}

/// `http://user:pass@proxy:3128` -> `http://user:***@proxy:3128`, `http://token@proxy` -> `http://***@proxy`
fn hide_password(url: &str) -> String {
    let start = url.find("://").map_or(0, |i| i + "://".len());
    let authority_end = url[start..].find('/').map_or(url.len(), |i| start + i);
    let Some(at) = url[start..authority_end].rfind('@').map(|i| start + i) else {
        return url.to_owned();
    };
    // Without a ':', userinfo is likely a bare token
    let user = url[start..at].split_once(':').map_or(String::new(), |(user, _)| format!("{user}:"));
    format!("{}{user}***{}", &url[..start], &url[at..])
}

#[test]
fn redacting() {
    assert_eq!(redact("GITHUB_TOKEN", "ghp_abcdefghijklmnop"), "ghp_a***");
    assert_eq!(redact("CARGO_REGISTRY_TOKEN", "cio1234567890"), "cio12***");
    assert_eq!(redact("BUILDX_BAKE_GIT_AUTH_TOKEN", "abcdefgh"), "abcd***");
    assert_eq!(redact("SOME_UNKNOWN_VAR", "abc"), "a***");
    assert_eq!(redact("SOME_UNKNOWN_VAR", ""), "");
    assert_eq!(redact("AWS_SECRET_ACCESS_KEY", "é"), "***");

    assert_eq!(redact("CARGO_PKG_NAME", "cargo-green"), "cargo-green");
    assert_eq!(redact("RUSTFLAGS", "-Copt-level=3"), "-Copt-level=3");
    assert_eq!(redact("DOCKER_HOST", "ssh://ter"), "ssh://ter");
    assert_eq!(redact("CARGOGREEN_LOG", "debug"), "debug");
    assert_eq!(redact("OUT_DIR", "/target/out"), "/target/out");

    assert_eq!(redact("HTTPS_PROXY", "http://me:hunter2@proxy:3128"), "http://me:***@proxy:3128");
    assert_eq!(redact("http_proxy", "me:hunter2@proxy:3128/x@y"), "me:***@proxy:3128/x@y");
    assert_eq!(redact("HTTP_PROXY", "http://token@proxy"), "http://***@proxy");
    assert_eq!(redact("NO_PROXY", "localhost,.corp"), "localhost,.corp");
    assert_eq!(redact("https_proxy", "http://proxy:3128"), "http://proxy:3128");

    assert_eq!(format!("{:?}", Redacted("GH_TOKEN", "0123456789")), "GH_TOKEN=01234***");
}
