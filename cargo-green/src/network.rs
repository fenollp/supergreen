//! Which network the `RUN`s calling `rustc` and build scripts get: none, but for the dependencies
//! granted it.
//!
//! This is set on each `RUN` and never with the runner's `--network` flag:
//! * BuildKit hashes the whole exec operation, network mode included, into a step's cache key.
//!   So `RUN --network=none` and `RUN` don't share results.
//! * `RUN --network=none` does share them with a plain `RUN` of a `--network=none` build.
//! * `RUN --network=default` can't undo a build-wide `--network=none`, so that flag is never set.
//!
//! The `RUN`s of base stages (installing the toolchain and OS packages) keep the default network.

use std::{collections::BTreeMap, fmt};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::pkg_exprs::{self, PkgExpr};

/// Ordered by how much is granted. Written in TOML as `network = true` or `false`.
#[derive(Debug, Copy, Clone, Default, Serialize, Deserialize, Eq, PartialEq, Ord, PartialOrd)]
#[serde(from = "bool", into = "bool")]
pub(crate) enum Network {
    #[default]
    None,
    Default,
}

impl From<bool> for Network {
    fn from(granted: bool) -> Self {
        if granted { Self::Default } else { Self::None }
    }
}

impl From<Network> for bool {
    fn from(network: Network) -> Self {
        network == Network::Default
    }
}

impl Network {
    /// What follows `RUN`
    #[must_use]
    pub(crate) fn run_flag(self) -> &'static str {
        match self {
            Self::None => " --network=none",
            Self::Default => "",
        }
    }
}

/// `[package.metadata.green.deps]`: settings of dependencies, by package name.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Deps(BTreeMap<String, Dep>);

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct Dep {
    /// For all versions
    #[serde(skip_serializing_if = "Option::is_none")]
    network: Option<Network>,

    /// For versions matching a requirement, written as in `Cargo.toml`'s `[dependencies]`
    #[serde(flatten)]
    versions: BTreeMap<String, DepVersions>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct DepVersions {
    network: Network,
}

impl Deps {
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// What `name@version` is granted: by the requirements it matches (the most, if many),
    /// otherwise by its name.
    #[must_use]
    pub(crate) fn network(&self, name: &str, version: &str) -> Network {
        let Some(Dep { network, versions }) = self.0.get(name) else { return Network::None };
        pkg_exprs::matching(versions, version)
            .map(|v| v.network)
            .max()
            .or(*network)
            .unwrap_or_default()
    }

    /// What each setting is about, granting or not.
    #[must_use]
    pub(crate) fn exprs(&self) -> Vec<PkgExpr> {
        self.0
            .iter()
            .flat_map(|(name, Dep { network, versions })| {
                let reqs = network.map(|_| None).into_iter();
                reqs.chain(versions.keys().cloned().map(Some))
                    .map(|req| PkgExpr { name: name.clone(), req })
            })
            .collect()
    }

    pub(crate) fn validate(&self, origin: &str) -> Result<()> {
        for (name, Dep { network, versions }) in &self.0 {
            if !pkg_exprs::is_pkg_name(name) {
                bail!("{origin} {name:?} is not a package name")
            }
            if network.is_none() && versions.is_empty() {
                bail!("{origin} sets nothing for {name:?}")
            }
            for req in versions.keys() {
                pkg_exprs::check_requirement(req, name, origin)?;
            }
        }
        Ok(())
    }

    /// Grants what each of the [`pkg_exprs`] listed is about.
    pub(crate) fn parse(val: &str, origin: &str) -> Result<Self> {
        let mut deps = Self::default();
        for PkgExpr { name, req } in pkg_exprs::parse_list(val, origin)? {
            let dep = deps.0.entry(name).or_default();
            match req {
                Some(req) => {
                    dep.versions.insert(req, DepVersions { network: Network::Default });
                }
                None => dep.network = Some(Network::Default),
            }
        }
        Ok(deps)
    }
}

/// As [`Deps::parse`] reads them. Grants only: what isn't granted isn't listed.
impl fmt::Display for Deps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut exprs = vec![];
        for (name, Dep { network, versions }) in &self.0 {
            if *network == Some(Network::Default) {
                exprs.push(PkgExpr { name: name.clone(), req: None });
            }
            for (req, v) in versions {
                if v.network == Network::Default {
                    exprs.push(PkgExpr { name: name.clone(), req: Some(req.clone()) });
                }
            }
        }
        write!(f, "{}", pkg_exprs::to_list(&exprs))
    }
}

#[cfg(test)]
mod who_gets_to_phone_home {
    use test_case::test_case;

    use super::{Deps, Network};
    use crate::pkg_exprs;

    fn deps() -> Deps {
        toml::from_str(
            r#"
[openssl-sys]
network = true
[zipsign-api.'0.1.5']
network = true
[something_else.'0, >=0.11']
network = true
[mixed]
network = true
[mixed.'=2.0.0']
network = false
[mixed.'^3']
network = true
"#,
        )
        .unwrap()
    }

    #[test_case("openssl-sys", "0.9.1", Network::Default; "any version")]
    #[test_case("openssl", "0.9.1", Network::None; "another package")]
    #[test_case("zipsign-api", "0.1.5", Network::Default; "that version")]
    #[test_case("zipsign-api", "0.1.9", Network::Default; "compatible, as in Cargo")]
    #[test_case("zipsign-api", "0.1.4", Network::None; "an older one")]
    #[test_case("zipsign-api", "0.2.0", Network::None; "an incompatible one")]
    #[test_case("something_else", "0.11.0", Network::Default; "within the range")]
    #[test_case("something_else", "0.10.0", Network::None; "below the range")]
    #[test_case("something_else", "1.0.0", Network::None; "above the range")]
    #[test_case("mixed", "1.0.0", Network::Default; "by name")]
    #[test_case("mixed", "2.0.0", Network::None; "denied by version")]
    #[test_case("mixed", "3.1.0", Network::Default; "granted by version")]
    fn grants(name: &str, version: &str, network: Network) {
        assert_eq!(deps().network(name, version), network);
    }

    #[test]
    fn every_setting_is_about_something() {
        assert_eq!(
            pkg_exprs::to_list(&deps().exprs()),
            "mixed;mixed@=2.0.0;mixed@^3;openssl-sys;something_else@0, >=0.11;zipsign-api@0.1.5"
        );
    }

    #[test]
    fn nothing_by_default() {
        assert_eq!(Deps::default().network("openssl-sys", "0.9.1"), Network::None);
    }

    #[test]
    fn env_values() {
        let val = "openssl-sys;zipsign-api@0.1.5;something_else@0, >=0.11;mixed@^3";
        let deps = Deps::parse(val, "$VAR").unwrap();
        assert_eq!(deps.network("openssl-sys", "0.9.1"), Network::Default);
        assert_eq!(deps.network("zipsign-api", "0.1.5"), Network::Default);
        assert_eq!(deps.network("something_else", "0.11.2"), Network::Default);
        assert_eq!(deps.network("something_else", "0.10.0"), Network::None);
        assert_eq!(deps.network("mixed", "3.0.0"), Network::Default);
        assert_eq!(
            deps.to_string(),
            "mixed@^3;openssl-sys;something_else@0, >=0.11;zipsign-api@0.1.5"
        );
        assert_eq!(Deps::parse(&deps.to_string(), "$VAR").unwrap(), deps);
    }

    #[test]
    fn bad_env_values() {
        let err = Deps::parse("a,b", "$VAR").unwrap_err().to_string();
        assert!(err.starts_with("$VAR "), "{err}");
    }

    #[test]
    fn survives_the_trip_to_wrappers() {
        let json = serde_json::to_string(&deps()).unwrap();
        assert_eq!(serde_json::from_str::<Deps>(&json).unwrap(), deps());
    }
}
