use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "kebab-case")]
pub(crate) struct Config {
    /// --buildkitd-flags '--debug'
    /// => docker logs $BUILDX_BUILDER -f | grep 'do request.+host='
    #[serde(skip_serializing_if = "<&bool as std::ops::Not>::not")]
    pub(crate) debug: bool,

    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) registry: BTreeMap<String, Registry>,

    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) worker: BTreeMap<String, Worker>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) insecure_entitlements: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "kebab-case")]
pub(crate) struct Registry {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) mirrors: Vec<String>,

    #[serde(skip_serializing_if = "<&bool as std::ops::Not>::not")]
    pub(crate) http: bool,

    #[serde(skip_serializing_if = "<&bool as std::ops::Not>::not")]
    pub(crate) insecure: bool,
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "kebab-case")]
pub(crate) struct Worker {
    pub(crate) max_parallelism: u8,
}

#[test]
fn mirrors() {
    let cfg = &r#"
debug = true

[registry."docker.io"]
mirrors = [
    "localhost:5000",
    "public.ecr.aws/docker",
]
"#[1..];

    let de: Config = toml::de::from_str(cfg).unwrap();
    assert_ne!(de, Config::default());
    assert_eq!(
        de,
        Config {
            debug: true,
            registry: [(
                "docker.io".to_owned(),
                Registry {
                    mirrors: vec!["localhost:5000".to_owned(), "public.ecr.aws/docker".to_owned()],
                    ..Default::default()
                }
            )]
            .into(),
            ..Default::default()
        }
    );

    let ser = toml::to_string_pretty(&de).unwrap();
    println!("{ser}");
    assert_eq!(ser, cfg);
}

#[test]
fn private_insecure_registry() {
    let cfg = &r#"
[registry."192.168.189.102:5000"]
insecure = true

[registry."localhost:5000"]
http = true
insecure = true
"#[1..];

    let de: Config = toml::de::from_str(cfg).unwrap();
    assert_eq!(
        de,
        Config {
            registry: [
                (
                    "192.168.189.102:5000".to_owned(),
                    Registry { insecure: true, ..Default::default() }
                ),
                (
                    "localhost:5000".to_owned(),
                    Registry { http: true, insecure: true, ..Default::default() }
                ),
            ]
            .into(),
            ..Default::default()
        }
    );

    let ser = toml::to_string_pretty(&de).unwrap();
    println!("{ser}");
    assert_eq!(ser, cfg);
}

#[test]
fn insecure() {
    let cfg = &r#"
insecure-entitlements = [
    "network.host",
    "security.insecure",
]
"#[1..];

    let de: Config = toml::de::from_str(cfg).unwrap();
    assert_eq!(
        de,
        Config {
            insecure_entitlements: vec!["network.host".to_owned(), "security.insecure".to_owned()],
            ..Default::default()
        }
    );

    let ser = toml::to_string_pretty(&de).unwrap();
    println!("{ser}");
    assert_eq!(ser, cfg);
}

#[test]
fn parallelism() {
    let cfg = &r#"
[worker.oci]
max-parallelism = 4
"#[1..];

    let de: Config = toml::de::from_str(cfg).unwrap();
    assert_eq!(
        de,
        Config {
            worker: [("oci".to_owned(), Worker { max_parallelism: 4 })].into(),
            ..Default::default()
        }
    );

    let ser = toml::to_string_pretty(&de).unwrap();
    println!("{ser}");
    assert_eq!(ser, cfg);
}
