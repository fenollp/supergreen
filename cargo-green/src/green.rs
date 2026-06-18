use std::{
    collections::{HashMap, HashSet},
    env,
};

use anyhow::{Result, anyhow, bail};
use camino::Utf8PathBuf;
use cargo_toml::Manifest;
use log::warn;
use serde::{Deserialize, Serialize};

use crate::{
    ENV_RUNNER, PKG,
    add::Add,
    base_image::BaseImage,
    builder::Builder,
    buildkitd::MIRRORS,
    cache::Cache,
    containerfile::Containerfile,
    dirs::Dirs,
    r#final::Final,
    image_uri::{BAD_CHARS, ImageUri},
    lockfile::find_manifest_path,
    runner::Runner,
};

macro_rules! ENV_REGISTRY_MIRRORS {
    () => {
        "CARGOGREEN_REGISTRY_MIRRORS"
    };
}

macro_rules! ENV_SET_ENVS {
    () => {
        "CARGOGREEN_SET_ENVS"
    };
}

#[macro_export]
macro_rules! ENV_SYNTAX_IMAGE {
    () => {
        "CARGOGREEN_SYNTAX_IMAGE"
    };
}

// from https://github.com/PRQL/prql/pull/3773/files
// [profile.release.package.prql-compiler]
// strip = "debuginfo"
//=> look into how `[profile.release.package.PACKAGE]` settings are propagated

// TODO: cli config / profiles https://github.com/rust-lang/cargo/wiki/Third-party-cargo-subcommands
//   * https://docs.rs/figment/latest/figment/
//   * https://lib.rs/crates/toml_edit
//   * https://github.com/jdrouet/serde-toml-merge
//   * https://crates.io/crates/toml-merge
// https://github.com/cbourjau/cargo-with
// https://github.com/RazrFalcon/cargo-bloat
// https://lib.rs/crates/cargo_metadata
// https://github.com/stormshield/cargo-ft/blob/d4ba5b048345ab4b21f7992cc6ed12afff7cc863/src/package/metadata.rs

// TODO? switch all envs to TOML: cargo --config 'build.rustdocflags = ["--html-in-header", "header.html"]' …

#[doc = include_str!("../docs/configuration.md")]
#[derive(Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct Green {
    #[doc = include_str!(concat!("../docs/",ENV_RUNNER!(),".md"))]
    pub(crate) runner: Runner,

    /// Memoized $CARGO_HOME
    #[doc(hidden)]
    pub(crate) cargo_home: Utf8PathBuf,

    /// Various paths. Not user-settable.
    #[doc(hidden)]
    pub(crate) dirs: Option<Dirs>,

    /// Snapshot of runner's envs. Not user-settable.
    #[doc(hidden)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub(crate) runner_envs: HashMap<String, String>,

    #[serde(flatten)]
    pub(crate) builder: Builder,

    #[doc = include_str!(concat!("../docs/",ENV_SYNTAX_IMAGE!(),".md"))]
    pub(crate) syntax: ImageUri,

    #[doc = include_str!(concat!("../docs/",ENV_REGISTRY_MIRRORS!(),".md"))]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) registry_mirrors: Vec<String>,

    #[serde(flatten)]
    pub(crate) cache: Cache,

    #[serde(flatten)]
    pub(crate) r#final: Final,

    #[serde(flatten)]
    pub(crate) base: BaseImage,

    #[doc = include_str!(concat!("../docs/",ENV_SET_ENVS!(),".md"))]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) set_envs: Vec<String>,

    #[serde(skip_serializing_if = "Add::is_empty")]
    pub(crate) add: Add,

    #[doc = include_str!(concat!("../docs/",ENV_EXPERIMENT!(),".md"))]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) experiment: Vec<String>,

    #[doc = include_str!(concat!("../docs/",ENV_COMPONENTS!(),".md"))]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) components: Vec<String>,
}

impl Green {
    pub(crate) fn new_containerfile(&self) -> Containerfile {
        Containerfile::with_syntax(&self.syntax)
    }

    // TODO: handle worskpace cfg + merging fields
    // TODO: find a way to read cfg on `cargo install <non-local code>` cc https://github.com/rust-lang/cargo/issues/9700#issuecomment-2748617896
    pub(crate) async fn new_from_env_then_manifest() -> Result<Self> {
        let manifest_path =
            find_manifest_path().await.map_err(|e| anyhow!("Can't find package manifest: {e}"))?;
        let manifest = Manifest::from_path(&manifest_path)
            .map_err(|e| anyhow!("Can't read package manifest {manifest_path}: {e}"))?;

        Self::try_new(manifest).map_err(|e| anyhow!("Failed reading {PKG} configuration: {e}"))
    }

    fn try_new(manifest: Manifest) -> Result<Self> {
        use cargo_toml::Package;

        let mut green = Self::default();

        if let Manifest { package: Some(Package { metadata: Some(metadata), .. }), .. } = manifest {
            #[derive(Deserialize, Default)]
            struct GreenMetadata {
                green: Option<Green>,
            }
            if let GreenMetadata { green: Some(from_manifest) } = metadata.try_into()? {
                green = from_manifest;
            }
        }

        let var = ENV_REGISTRY_MIRRORS!();
        let mut origin = setting(var);
        let mut was_reset = false;
        if let Ok(val) = env::var(var) {
            origin = format!("${var}");
            if val.is_empty() {
                was_reset = true;
                green.registry_mirrors = vec![];
            } else {
                green.registry_mirrors = parse_csv(&val);
            }
        }
        if green.registry_mirrors.len()
            != green.registry_mirrors.iter().collect::<HashSet<_>>().len()
        {
            bail!("{origin} contains duplicates")
        }
        if green.registry_mirrors.is_empty() && !was_reset {
            green.registry_mirrors = MIRRORS.iter().map(ToString::to_string).collect();
        }

        for (field, var) in [
            (&mut green.cache.from_images, ENV_CACHE_FROM_IMAGES!()),
            (&mut green.cache.to_images, ENV_CACHE_TO_IMAGES!()),
            (&mut green.cache.images, ENV_CACHE_IMAGES!()),
        ] {
            let mut origin = setting(var);
            if let Ok(val) = env::var(var) {
                origin = format!("${var}");
                *field = val
                    .split(',')
                    .map(|x| ImageUri::try_new(x).map_err(|e| anyhow!("{origin} {e}")))
                    .collect::<Result<_>>()?;
            }
            if field.len() != field.iter().collect::<HashSet<_>>().len() {
                bail!("{origin} contains duplicates")
            }
            for item in field {
                if !item.noscheme().contains('/') {
                    bail!("{origin} must contain a registry and namespace: {item:?}")
                }
                if item.tagged() || item.locked() {
                    bail!("{origin} must not contain a tag nor digest: {item:?}")
                }
            }
        }

        for (field, var) in
            [(&mut green.add.apk, ENV_ADD_APK!()), (&mut green.add.apt, ENV_ADD_APT!())]
        {
            let origin = validate_csv(field, var)?;
            for f in field.iter().filter(|f| !f.contains('=')) {
                warn!("warning: config {origin} is missing version constraints on {f:?}");
                eprintln!("warning: config {origin} is missing version constraints on {f:?}");
            }
        }

        validate_csv(&mut green.components, ENV_COMPONENTS!())?;

        if !green.base.image_inline.is_empty() {
            bail!("'base-image-inline' setting cannot be set")
        }
        let var = ENV_BASE_IMAGE!();
        if let Ok(val) = env::var(var) {
            green.base.image = val.try_into().map_err(|e| anyhow!("${var} {e}"))?;
        }

        validate_csv(&mut green.set_envs, ENV_SET_ENVS!())?;
        if green.set_envs.iter().any(|var| var.starts_with("CARGOGREEN_")) {
            bail!("{origin} contains CARGOGREEN_* names")
        }

        Ok(green)
    }
}

fn env_as_toml(var: &str) -> String {
    var.replace("CARGOGREEN_", "").replace('_', "-").to_lowercase()
}

fn setting(var: &str) -> String {
    format!("[metadata.green.{}]", env_as_toml(var))
}

fn parse_csv(val: &str) -> Vec<String> {
    val.split(',').map(ToOwned::to_owned).collect()
}

pub(crate) fn validate_csv(field: &mut Vec<String>, var: &'static str) -> Result<String> {
    let mut origin = setting(var);
    if let Ok(val) = env::var(var) {
        origin = format!("${var}");
        if val.is_empty() {
            bail!("{origin} is empty")
        }

        *field = parse_csv(&val);
    }
    if !field.is_empty() {
        if field.iter().any(|x| x.is_empty() || x.contains(BAD_CHARS) || x.trim() != x) {
            bail!("{origin} contains empty names, whitespace, quotes or bad characters")
        }

        if field.len() != field.iter().collect::<HashSet<_>>().len() {
            bail!("{origin} contains duplicates")
        }
    }
    Ok(origin)
}

#[cfg(test)]
mod test_metadata {
    mod green {
        use super::super::{Green, Manifest};
        use crate::base_image::BaseImage;

        #[test_case::test_matrix(["", "[package.metadata.green]", "[package.metadata.other]"])]
        fn ok(conf: &str) {
            let manifest = Manifest::from_str(&format!(
                r#"
[package]
name = "test-package"

{conf}
"#
            ))
            .unwrap();
            let mut green = Green::try_new(manifest).unwrap();

            assert_eq!(green.base, BaseImage::default());

            assert!(!green.registry_mirrors.is_empty());
            green.registry_mirrors = vec![];

            assert_eq!(green, Green::default());
        }
    }

    mod components {
        use super::super::{Green, Manifest};

        #[test]
        fn ok() {
            let manifest = Manifest::from_str(
                r#"
[package]
name = "test-package"

[package.metadata.green]
components = [ "rust-src", "llvm-tools-preview" ]
"#,
            )
            .unwrap();
            let green = Green::try_new(manifest).unwrap();
            assert_eq!(
                green.components,
                vec!["rust-src".to_owned(), "llvm-tools-preview".to_owned()]
            );
        }

        #[test]
        fn empty_name() {
            let manifest = Manifest::from_str(
                r#"
[package]
name = "test-package"

[package.metadata.green]
components = [ "" ]
"#,
            )
            .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("empty"), "In: {err}");
        }

        #[test]
        fn quotes() {
            let manifest = Manifest::from_str(
                r#"
[package]
name = "test-package"

[package.metadata.green]
components = [ "'a'" ]
"#,
            )
            .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("quotes"), "In: {err}");
        }

        #[test]
        fn whitespace() {
            let manifest = Manifest::from_str(
                r#"
[package]
name = "test-package"

[package.metadata.green]
components = [ "a b" ]
"#,
            )
            .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("space"), "In: {err}");
        }

        #[test]
        fn duplicates() {
            let manifest = Manifest::from_str(
                r#"
[package]
name = "test-package"

[package.metadata.green]
components = [ "a", "b", "a" ]
            "#,
            )
            .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("duplicates"), "In: {err}");
        }
    }

    mod add {
        use super::super::{Green, Manifest};

        #[test]
        fn ok() {
            let manifest = Manifest::from_str(
                r#"
[package]
name = "test-package"

[package.metadata.green]
add.apt = [ "libpq-dev", "pkg-config" ]
add.apk = [ "libpq-dev", "pkgconf" ]
"#,
            )
            .unwrap();
            let green = Green::try_new(manifest).unwrap();
            assert_eq!(green.add.apt, vec!["libpq-dev".to_owned(), "pkg-config".to_owned()]);
            assert_eq!(green.add.apk, vec!["libpq-dev".to_owned(), "pkgconf".to_owned()]);
        }

        #[test]
        fn empty_var() {
            use crate::green::{parse_csv, validate_csv};
            let var = ENV_ADD_APT!();
            temp_env::with_var(var, Some("a=1,,b"), || {
                let mut field = parse_csv(&std::env::var(var).unwrap());
                let err = validate_csv(&mut field, var).err().unwrap().to_string();
                assert!(err.contains("empty"), "In: {err}");
                assert!(err.contains(&format!("${}", var)), "In: {err}");
            });
        }

        #[test_case::test_matrix(["apt", "apk"])]
        fn empty_name(setting: &str) {
            let manifest = Manifest::from_str(&format!(
                r#"
[package]
name = "test-package"

[package.metadata.green]
add.{setting} = [ "" ]
"#
            ))
            .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("empty"), "In: {err}");
        }

        #[test_case::test_matrix(["apt", "apk"])]
        fn quotes(setting: &str) {
            let manifest = Manifest::from_str(&format!(
                r#"
[package]
name = "test-package"

[package.metadata.green]
add.{setting} = [ "'a'" ]
"#
            ))
            .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("quotes"), "In: {err}");
        }

        #[test_case::test_matrix(["apt", "apk"])]
        fn whitespace(setting: &str) {
            let manifest = Manifest::from_str(&format!(
                r#"
[package]
name = "test-package"

[package.metadata.green]
add.{setting} = [ "a b" ]
"#
            ))
            .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("space"), "In: {err}");
        }

        #[test_case::test_matrix(["apt", "apk"])]
        fn duplicates(setting: &str) {
            let manifest = Manifest::from_str(&format!(
                r#"
[package]
name = "test-package"

[package.metadata.green]
add.{setting} = [ "a", "b", "a" ]
            "#
            ))
            .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("duplicates"), "In: {err}");
        }
    }

    mod set_envs {
        use super::super::{Green, Manifest};

        #[test]
        fn ok() {
            let manifest = Manifest::from_str(
                r#"
[package]
name = "test-package"

[package.metadata.green]
set-envs = [ "GIT_AUTH_TOKEN", "TYPENUM_BUILD_CONSTS", "TYPENUM_BUILD_OP" ]
"#,
            )
            .unwrap();
            let green = Green::try_new(manifest).unwrap();
            assert_eq!(
                green.set_envs,
                vec![
                    "GIT_AUTH_TOKEN".to_owned(),
                    "TYPENUM_BUILD_CONSTS".to_owned(),
                    "TYPENUM_BUILD_OP".to_owned()
                ]
            );
        }

        #[test]
        fn empty_var() {
            let manifest = Manifest::from_str(
                r#"
[package]
name = "test-package"

[package.metadata.green]
set-envs = [ "" ]
"#,
            )
            .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("empty name"), "In: {err}");
        }

        #[test]
        fn quotes() {
            let manifest = Manifest::from_str(
                r#"
[package]
name = "test-package"

[package.metadata.green]
set-envs = [ "'a'" ]
"#,
            )
            .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("quotes"), "In: {err}");
        }

        #[test]
        fn whitespace() {
            let manifest = Manifest::from_str(
                r#"
[package]
name = "test-package"

[package.metadata.green]
set-envs = [ "A B" ]
"#,
            )
            .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("space"), "In: {err}");
        }

        #[test]
        fn our_vars() {
            let manifest = Manifest::from_str(
                r#"
[package]
name = "test-package"

[package.metadata.green]
set-envs = [ "CARGOGREEN_LOG" ]
"#,
            )
            .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("CARGOGREEN"), "In: {err}");
        }

        #[test]
        fn duplicates() {
            let manifest = Manifest::from_str(
                r#"
[package]
name = "test-package"

[package.metadata.green]
set-envs = [ "A", "B", "A" ]
"#,
            )
            .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("duplicates"), "In: {err}");
        }
    }

    mod base {
        use super::super::{Green, Manifest};
        use crate::{base_image::BaseImage, image_uri::ImageUri, network::Network};

        #[test]
        fn ok() {
            let manifest = Manifest::from_str(
                r#"
[package]
name = "test-package"

[package.metadata.green]
base-image = "docker-image://docker.io/library/ubuntu:latest"
"#,
            )
            .unwrap();
            let green = Green::try_new(manifest).unwrap();
            assert_eq!(
                green.base,
                BaseImage { image: ImageUri::std("ubuntu:latest"), ..Default::default() }
            );
        }

        #[test]
        fn with_network_ok() {
            let manifest = Manifest::from_str(
                r#"
[package]
name = "test-package"

[package.metadata.green]
with-network = "default"
base-image = "docker-image://docker.io/library/ubuntu:latest"
"#,
            )
            .unwrap();
            let green = Green::try_new(manifest).unwrap();
            assert_eq!(
                green.base,
                BaseImage {
                    image: ImageUri::std("ubuntu:latest"),
                    with_network: Network::Default,
                    ..Default::default()
                }
            );
        }

        #[test]
        fn empty() {
            let manifest = Manifest::from_str(
                r#"
[package]
name = "test-package"

[package.metadata.green]
base-image = ""
"#,
            )
            .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("scheme"), "In: {err}");
        }

        #[test]
        fn bad_scheme() {
            let manifest = Manifest::from_str(
                r#"
[package]
name = "test-package"

[package.metadata.green]
base-image = "docker.io/library/ubuntu:latest"
"#,
            )
            .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("scheme"), "In: {err}");
        }

        #[test]
        fn whitespace() {
            let manifest = Manifest::from_str(
                r#"
[package]
name = "test-package"

[package.metadata.green]
base-image = " docker-image://docker.io/library/ubuntu:latest  "
"#,
            )
            .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("space"), "In: {err}");
        }
    }

    mod cache_images {
        use super::super::{Green, Manifest};
        use crate::image_uri::ImageUri;

        #[test_case::test_matrix(["cache-images", "cache-from-images", "cache-to-images"])]
        fn ok(setting: &str) {
            let manifest = Manifest::from_str(&format!(
                r#"
[package]
name = "test-package"

[package.metadata.green]
{setting} = [
  "docker-image://some-registry.com/dir/image",
  "docker-image://other.registry/dir2/image3",
]
"#,
            ))
            .unwrap();
            let green = Green::try_new(manifest).unwrap();
            assert_eq!(
                match setting {
                    "cache-images" => green.cache.images,
                    "cache-from-images" => green.cache.from_images,
                    "cache-to-images" => green.cache.to_images,
                    _ => unreachable!(),
                },
                vec![
                    ImageUri::try_new("docker-image://some-registry.com/dir/image").unwrap(),
                    ImageUri::try_new("docker-image://other.registry/dir2/image3").unwrap(),
                ]
            );
        }

        #[test_case::test_matrix(["cache-images", "cache-from-images", "cache-to-images"])]
        fn dupes(setting: &str) {
            let manifest = Manifest::from_str(&format!(
                r#"
[package]
name = "test-package"

[package.metadata.green]
{setting} = [
  "docker-image://some-registry.com/dir/image",
  "docker-image://other.registry/dir2/image3",
  "docker-image://some-registry.com/dir/image",
]
"#,
            ))
            .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("duplicates"), "In: {err}");
        }

        #[test_case::test_matrix(["cache-images", "cache-from-images", "cache-to-images"])]
        fn bad_names(setting: &str) {
            let manifest = Manifest::from_str(&format!(
        r#"
[package]
name = "test-package"

[package.metadata.green]
{setting} = ["docker-image://some-registry.com/dir/image 'docker-image://other.registry/dir2/image3'", ""]
"#,
    ))
    .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("names"), "In: {err}");
        }

        #[test_case::test_matrix(["cache-images", "cache-from-images", "cache-to-images"])]
        fn bad_scheme(setting: &str) {
            let manifest = Manifest::from_str(&format!(
                r#"
[package]
name = "test-package"

[package.metadata.green]
{setting} = ["some-registry.com/dir/image"]
"#,
            ))
            .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("scheme"), "In: {err}");
        }

        #[test_case::test_matrix(["cache-images", "cache-from-images", "cache-to-images"])]
        fn bad_registry(setting: &str) {
            let manifest = Manifest::from_str(&format!(
                r#"
[package]
name = "test-package"

[package.metadata.green]
{setting} = ["docker-image://image"]
"#,
            ))
            .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("registry"), "In: {err}");
        }

        #[test_case::test_matrix(["cache-images", "cache-from-images", "cache-to-images"])]
        fn bad_image(setting: &str) {
            let manifest = Manifest::from_str(&format!(
                r#"
[package]
name = "test-package"

[package.metadata.green]
{setting} = ["docker-image://some-registry.com/dir/image:sometag"]
"#,
            ))
            .unwrap();
            let err = Green::try_new(manifest).err().unwrap().to_string();
            assert!(err.contains("tag"), "In: {err}");
        }
    }
}
