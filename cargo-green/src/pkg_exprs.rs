//! Expressions saying which packages a setting is about.
//!
//! `name[@requirement]` is the package `name`: all of its versions, or those matching a requirement.
//!
//! Requirements are Cargo's, as in `Cargo.toml`'s `[dependencies]` (`0.1.5` means `>=0.1.5, <0.2.0`,
//! pre-releases only match when asked for). They are parsed and matched by `semver`, as Cargo does:
//! <https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#version-requirement-syntax>
//!
//! So `name@requirement` reads as with `cargo add`, but not as a package ID spec, where `name@1.4`
//! means `1.4.*` rather than `^1.4`: <https://doc.rust-lang.org/cargo/reference/pkgid-spec.html>
//!
//! Package names are those Cargo accepts, not only ASCII ones.
//!
//! Lists of them are `;`-separated: requirements hold commas (`0, >=0.11`), while neither they nor
//! package names can hold a `;`.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use anyhow::{Result, anyhow, bail};
use semver::{Version, VersionReq};
use unicode_ident::{is_xid_continue, is_xid_start};

const SEPARATOR: char = ';';

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PkgExpr {
    pub(crate) name: String,
    pub(crate) req: Option<String>,
}

impl fmt::Display for PkgExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { name, req } = self;
        match req {
            Some(req) => write!(f, "{name}@{req}"),
            None => write!(f, "{name}"),
        }
    }
}

impl PkgExpr {
    #[must_use]
    fn is_about(&self, name: &str, version: &Version) -> bool {
        self.name == name
            && self
                .req
                .as_ref()
                .is_none_or(|req| VersionReq::parse(req).is_ok_and(|req| req.matches(version)))
    }
}

/// The expressions that are about none of these packages.
#[must_use]
pub(crate) fn unmatched(
    mut exprs: Vec<PkgExpr>,
    packages: &BTreeSet<(String, Version)>,
) -> Vec<PkgExpr> {
    exprs.retain(|expr| !packages.iter().any(|(name, version)| expr.is_about(name, version)));
    exprs
}

/// Parses a list of expressions, each given once.
pub(crate) fn parse_list(val: &str, origin: &str) -> Result<Vec<PkgExpr>> {
    let mut seen = BTreeSet::new();
    val.split(SEPARATOR)
        .map(|expr| {
            let (name, req) = match expr.split_once('@') {
                Some((name, req)) => (name, Some(req)),
                None => (expr, None),
            };
            if !is_pkg_name(name) {
                bail!("{origin} {expr:?} is not a package name")
            }
            if let Some(req) = req {
                check_requirement(req, name, origin)?;
            }
            let expr = PkgExpr { name: name.to_owned(), req: req.map(ToOwned::to_owned) };
            if !seen.insert(expr.clone()) {
                bail!("{origin} contains duplicates")
            }
            Ok(expr)
        })
        .collect()
}

/// Cargo's rule: an identifier as Unicode defines them, that may also start with `_` or hold `-`.
#[must_use]
pub(crate) fn is_pkg_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| is_xid_start(c) || c == '_')
        && chars.all(|c| is_xid_continue(c) || c == '-')
}

/// As [`parse_list`] reads them.
#[must_use]
pub(crate) fn to_list(exprs: &[PkgExpr]) -> String {
    let exprs: Vec<_> = exprs.iter().map(ToString::to_string).collect();
    exprs.join(&SEPARATOR.to_string())
}

pub(crate) fn check_requirement(req: &str, name: &str, origin: &str) -> Result<()> {
    VersionReq::parse(req)
        .map(drop)
        .map_err(|e| anyhow!("{origin} {name:?} has a bad version requirement {req:?}: {e}"))
}

/// The values of the requirements that `version` matches.
pub(crate) fn matching<'a, T>(
    by_requirement: &'a BTreeMap<String, T>,
    version: &str,
) -> impl Iterator<Item = &'a T> {
    let version = Version::parse(version).ok();
    by_requirement.iter().filter_map(move |(req, value)| {
        let req = VersionReq::parse(req).ok()?;
        req.matches(version.as_ref()?).then_some(value)
    })
}

#[cfg(test)]
mod which_ones {
    use std::collections::{BTreeMap, BTreeSet};

    use semver::{Version, VersionReq};
    use test_case::test_case;

    use super::{PkgExpr, is_pkg_name, matching, parse_list, to_list, unmatched};

    #[test]
    fn lists_round_trip() {
        let val = "openssl-sys;zipsign-api@0.1.5;something_else@0, >=0.11;mixed@^3";
        let exprs = parse_list(val, "$VAR").unwrap();
        let expr = |name: &str, req: Option<&str>| PkgExpr {
            name: name.to_owned(),
            req: req.map(ToOwned::to_owned),
        };
        assert_eq!(
            exprs,
            [
                expr("openssl-sys", None),
                expr("zipsign-api", Some("0.1.5")),
                expr("something_else", Some("0, >=0.11")),
                expr("mixed", Some("^3")),
            ]
        );
        assert_eq!(to_list(&exprs), val);
    }

    #[test_case("serde"; "ascii")]
    #[test_case("_a-1"; "underscore first")]
    #[test_case("café"; "accented")]
    #[test_case("包"; "ideograms")]
    fn names(name: &str) {
        assert!(is_pkg_name(name));
        assert_eq!(parse_list(name, "$VAR").unwrap()[0].name, name);
    }

    #[test_case(""; "empty")]
    #[test_case("1a"; "digit first")]
    #[test_case("-a"; "dash first")]
    #[test_case("a🦀"; "emoji")]
    #[test_case("a;"; "trailing separator")]
    #[test_case("a;a"; "twice")]
    #[test_case("a,b"; "comma separated")]
    #[test_case("a@"; "no requirement")]
    #[test_case("a@nope"; "bad requirement")]
    #[test_case("a:host"; "suffix")]
    #[test_case("a b"; "not a name")]
    fn bad_lists(val: &str) {
        let err = parse_list(val, "$VAR").unwrap_err().to_string();
        assert!(err.starts_with("$VAR "), "{err}");
    }

    #[test_case(";"; "alone")]
    #[test_case("1;"; "after a version")]
    #[test_case("1;2"; "between versions")]
    #[test_case("1.0.0-a;b"; "in a pre release")]
    #[test_case("1.0.0+a;b"; "in build metadata")]
    fn requirements_never_hold_the_separator(req: &str) {
        assert!(VersionReq::parse(req).is_err());
    }

    #[test]
    fn all_that_match_nothing() {
        let packages = BTreeSet::from(
            [("a", "1.2.0"), ("a", "2.0.0"), ("b", "0.1.0")]
                .map(|(name, version)| (name.to_owned(), Version::parse(version).unwrap())),
        );
        let exprs = parse_list("a;a@1;a@=2.0.0;a@3;b@0.1;b@0.2;c;c@1", "$VAR").unwrap();
        assert_eq!(to_list(&unmatched(exprs, &packages)), "a@3;b@0.2;c;c@1");
    }

    /// The examples of <https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html>
    #[test_case("1.2.3", &["1.2.3", "1.9.0"], &["1.2.2", "2.0.0"]; "default")]
    #[test_case("1.2", &["1.2.0", "1.9.0"], &["1.1.9", "2.0.0"]; "default minor")]
    #[test_case("1", &["1.0.0", "1.9.0"], &["0.9.0", "2.0.0"]; "default major")]
    #[test_case("0.2.3", &["0.2.3", "0.2.9"], &["0.2.2", "0.3.0"]; "default zero major")]
    #[test_case("0.0.3", &["0.0.3"], &["0.0.2", "0.0.4"]; "default zero minor")]
    #[test_case("0.0", &["0.0.0", "0.0.9"], &["0.1.0"]; "default zero zero")]
    #[test_case("0", &["0.0.0", "0.9.0"], &["1.0.0"]; "default zero")]
    #[test_case("^1.2.3", &["1.2.3", "1.9.0"], &["1.2.2", "2.0.0"]; "caret")]
    #[test_case("~1.2.3", &["1.2.3", "1.2.9"], &["1.2.2", "1.3.0"]; "tilde")]
    #[test_case("~1.2", &["1.2.0", "1.2.9"], &["1.3.0"]; "tilde minor")]
    #[test_case("~1", &["1.0.0", "1.9.0"], &["2.0.0"]; "tilde major")]
    #[test_case("*", &["0.0.0", "9.0.0"], &[]; "wildcard")]
    #[test_case("1.*", &["1.0.0", "1.9.0"], &["0.9.0", "2.0.0"]; "wildcard minor")]
    #[test_case("1.2.*", &["1.2.0", "1.2.9"], &["1.1.9", "1.3.0"]; "wildcard patch")]
    #[test_case(">= 1.2.0", &["1.2.0", "2.0.0"], &["1.1.9"]; "at least")]
    #[test_case("> 1", &["2.0.0"], &["1.9.0"]; "above")]
    #[test_case("< 2", &["1.9.0"], &["2.0.0"]; "below")]
    #[test_case("= 1.2.3", &["1.2.3"], &["1.2.4"]; "exactly")]
    #[test_case(">= 1.2, < 1.5", &["1.2.0", "1.4.9"], &["1.1.9", "1.5.0"]; "multiple")]
    #[test_case("1.0", &["1.0.0"], &["1.0.0-alpha", "1.1.0-alpha"]; "no pre release")]
    #[test_case("1.0.0-alpha", &["1.0.0-beta", "1.0.0", "1.2.0"], &["1.0.1-alpha"]; "pre release")]
    #[test_case("1.0.0", &["1.0.0+21AF26D3"], &[]; "build metadata")]
    fn means_what_it_does_to_cargo(req: &str, matched: &[&str], unmatched: &[&str]) {
        let by_requirement = BTreeMap::from([(req.to_owned(), ())]);
        let matches = |version: &&str| matching(&by_requirement, version).next().is_some();
        for version in matched {
            assert!(matches(version), "{req:?} must match {version}");
        }
        for version in unmatched {
            assert!(!matches(version), "{req:?} must not match {version}");
        }
    }

    #[test_case("0.1.5", &["compatible", "range"]; "both")]
    #[test_case("0.1.4", &["range"]; "one")]
    #[test_case("1.0.0", &[]; "none")]
    #[test_case("not a version", &[]; "unreadable")]
    fn matches_requirements(version: &str, expected: &[&str]) {
        let by_requirement =
            BTreeMap::from([("0.1.5".to_owned(), "compatible"), ("<0.2".to_owned(), "range")]);
        let matched: Vec<_> = matching(&by_requirement, version).copied().collect();
        assert_eq!(matched, expected);
    }
}
