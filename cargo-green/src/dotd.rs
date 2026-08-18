//! Tools around cargo's dep-info files

/// Is this a dep-info filename
#[must_use]
pub(crate) fn is_dotd(fname: impl AsRef<str>) -> bool {
    fname.as_ref().ends_with(".d")
}

/// Produce a dep-info dependency on an environment variable binding
#[must_use]
pub(crate) fn env_dep(key: &str, val: &str) -> String {
    format!("# env-dep:{key}={val}\n")
}
