use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Default docker.io mirrors: "mirror.gcr.io", "public.ecr.aws/docker".
///
/// Hit me if you have more!
pub(crate) const MIRRORS: &[&str] = &["mirror.gcr.io", "public.ecr.aws/docker"];

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "kebab-case")]
pub(crate) struct Config {
    /// --buildkitd-flags '--debug'
    /// => docker logs $BUILDX_BUILDER -f | grep 'do request.+host='
    #[serde(skip_serializing_if = "<&bool as std::ops::Not>::not")]
    pub(crate) debug: bool,

    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub(crate) registry: IndexMap<String, Registry>,

    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub(crate) worker: IndexMap<String, Worker>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) insecure_entitlements: Vec<String>,
}

impl Config {
    pub(crate) fn set_registry_mirrors(&mut self, ns: &str, mirrors: Vec<String>) {
        let mirrors = Registry { mirrors, ..Default::default() };
        self.registry.insert(ns.to_owned(), mirrors);
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) enabled: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_parallelism: Option<u8>,

    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) namespace: String,
}

#[test]
fn default_cfg() {
    let cfg = Config::default();
    let ser = toml::to_string_pretty(&cfg).unwrap();
    assert_eq!(ser, "");
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
            worker: [("oci".to_owned(), Worker { max_parallelism: Some(4), ..Default::default() })]
                .into(),
            ..Default::default()
        }
    );

    let ser = toml::to_string_pretty(&de).unwrap();
    println!("{ser}");
    assert_eq!(ser, cfg);

    let cfg = &r#"
[worker.oci]
max-parallelism = 0
"#[1..];

    let de: Config = toml::de::from_str(cfg).unwrap();
    assert_eq!(
        de,
        Config {
            worker: [("oci".to_owned(), Worker { max_parallelism: Some(0), ..Default::default() })]
                .into(),
            ..Default::default()
        }
    );

    let ser = toml::to_string_pretty(&de).unwrap();
    println!("{ser}");
    assert_eq!(ser, cfg);

    let cfg =
        Config { worker: [("oci".to_owned(), Worker::default())].into(), ..Default::default() };

    let ser = toml::to_string_pretty(&cfg).unwrap();
    assert_eq!(
        ser,
        &r#"
[worker.oci]
"#[1..]
    );
}

// https://github.com/moby/buildkit/issues/5340#issuecomment-2828164139
#[test]
fn use_containerd() {
    let cfg = &r#"
[worker.containerd]
enabled = true
namespace = "default"

[worker.oci]
enabled = false
"#[1..];

    let de: Config = toml::de::from_str(cfg).unwrap();
    assert_eq!(
        de,
        Config {
            worker: [
                ("oci".to_owned(), Worker { enabled: Some(false), ..Default::default() }),
                (
                    "containerd".to_owned(),
                    Worker {
                        enabled: Some(true),
                        namespace: "default".to_owned(),
                        ..Default::default()
                    }
                )
            ]
            .into(),
            ..Default::default()
        }
    );

    let ser = toml::to_string_pretty(&de).unwrap();
    println!("{ser}");
    assert_eq!(ser, cfg);
}
