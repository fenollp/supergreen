use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow, bail};
use cargo_toml::{Manifest, Package, Value as MetadataValue};
use log::warn;
use serde::{Deserialize, Serialize};

use crate::{
    PKG,
    add::Add,
    all_our_envs::{self},
    base_image::BaseImage,
    builder::Builder,
    buildkitd::MIRRORS,
    cache::Cache,
    containerfile::Containerfile,
    dirs::Paths,
    r#final::Final,
    image_uri::{BAD_CHARS, ImageUri},
    lockfile::find_manifest_path,
    runner::Runner,
    wrap::Vars,
};

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
    /// On when cargo -vv / --verbose. Not user-settable.
    #[doc(hidden)]
    pub(crate) verbose: bool,

    #[doc = envdocs!(CARGOGREEN_RUNNER)]
    pub(crate) runner: Runner,

    /// Various paths. Not user-settable.
    #[doc(hidden)]
    pub(crate) paths: Paths,

    /// Snapshot of runner's envs. Not user-settable.
    #[doc(hidden)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub(crate) runner_envs: HashMap<String, String>,

    #[serde(flatten)]
    pub(crate) builder: Builder,

    #[doc = envdocs!(CARGOGREEN_SYNTAX_IMAGE)]
    pub(crate) syntax: ImageUri,

    #[doc = envdocs!(CARGOGREEN_REGISTRY_MIRRORS)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) registry_mirrors: Vec<String>,

    #[serde(flatten)]
    pub(crate) cache: Cache,

    #[serde(flatten)]
    pub(crate) r#final: Final,

    #[serde(flatten)]
    pub(crate) base: BaseImage,

    #[doc = envdocs!(CARGOGREEN_SET_ENVS)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) set_envs: Vec<String>,

    #[serde(skip_serializing_if = "Add::is_empty")]
    pub(crate) add: Add,

    #[doc = envdocs!(CARGOGREEN_EXPERIMENT)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) experiment: Vec<String>,

    #[doc = envdocs!(CARGOGREEN_COMPONENTS)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) components: Vec<String>,
}

impl Green {
    pub(crate) fn new_containerfile(&self) -> Containerfile {
        Containerfile::with_syntax(&self.syntax)
    }

    // TODO: handle worskpace cfg + merging fields
    // TODO: find a way to read cfg on `cargo install <non-local code>` cc https://github.com/rust-lang/cargo/issues/9700#issuecomment-2748617896
    pub(crate) async fn new_from_env_then_manifest(is_install: bool, vars: &Vars) -> Result<Self> {
        let manifest = if is_install {
            let empty_manifest: Manifest<MetadataValue> = Manifest::from_str("").unwrap();
            empty_manifest
        } else {
            let manifest_path = find_manifest_path()
                .await
                .map_err(|e| anyhow!("Can't find package manifest: {e}"))?;
            Manifest::from_path(&manifest_path)
                .map_err(|e| anyhow!("Can't read package manifest {manifest_path}: {e}"))?
        };

        Self::try_new(manifest, vars)
            .map_err(|e| anyhow!("Failed reading {PKG} configuration: {e}"))
    }

    fn try_new(manifest: Manifest, vars: &Vars) -> Result<Self> {
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

        let var = CARGOGREEN_REGISTRY_MIRRORS!();
        let mut origin = setting(var);
        let mut was_reset = false;
        if let Some(val) = vars.get(var) {
            origin = format!("${var}");
            if val.is_empty() {
                was_reset = true;
                green.registry_mirrors = vec![];
            } else {
                green.registry_mirrors = parse_csv(val);
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
            (&mut green.cache.from_images, CARGOGREEN_CACHE_FROM_IMAGES!()),
            (&mut green.cache.to_images, CARGOGREEN_CACHE_TO_IMAGES!()),
            (&mut green.cache.images, CARGOGREEN_CACHE_IMAGES!()),
        ] {
            let mut origin = setting(var);
            if let Some(val) = vars.get(var) {
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

        for (field, var) in [
            (&mut green.add.apk, CARGOGREEN_ADD_APK!()),
            (&mut green.add.apt, CARGOGREEN_ADD_APT!()),
        ] {
            let origin = validate_csv(field, var, vars)?;
            for f in field.iter().filter(|f| !f.contains('=')) {
                warn!("warning: config {origin} is missing version constraints on {f:?}");
                eprintln!("warning: config {origin} is missing version constraints on {f:?}");
            }
        }

        validate_csv(&mut green.components, CARGOGREEN_COMPONENTS!(), vars)?;

        if !green.base.image_inline.is_empty() {
            bail!("'base-image-inline' setting cannot be set")
        }
        let var = CARGOGREEN_BASE_IMAGE!();
        if let Some(val) = vars.get(var) {
            green.base.image = val.as_str().try_into().map_err(|e| anyhow!("${var} {e}"))?;
        }

        validate_csv(&mut green.set_envs, CARGOGREEN_SET_ENVS!(), vars)?;
        if green.set_envs.iter().any(|var| var.starts_with(all_our_envs::PREFIX)) {
            bail!("{origin} contains {}* names", all_our_envs::PREFIX)
        }

        Ok(green)
    }
}

fn env_as_toml(var: &str) -> String {
    var.replace(all_our_envs::PREFIX, "").replace('_', "-").to_lowercase()
}

fn setting(var: &str) -> String {
    format!("[metadata.green.{}]", env_as_toml(var))
}

fn parse_csv(val: &str) -> Vec<String> {
    val.split(',').map(ToOwned::to_owned).collect()
}

pub(crate) fn validate_csv(
    field: &mut Vec<String>,
    var: &'static str,
    vars: &Vars,
) -> Result<String> {
    let mut origin = setting(var);
    if let Some(val) = vars.get(var) {
        origin = format!("${var}");
        if val.is_empty() {
            bail!("{origin} is empty")
        }

        *field = parse_csv(val);
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
            let mut green = Green::try_new(manifest, &[].into()).unwrap();

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
            let green = Green::try_new(manifest, &[].into()).unwrap();
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
            assert!(err.contains("duplicates"), "In: {err}");
        }
    }

    mod add {
        use super::super::{Green, Manifest, all_our_envs::CARGOGREEN_ADD_APT};

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
            let green = Green::try_new(manifest, &[].into()).unwrap();
            assert_eq!(green.add.apt, vec!["libpq-dev".to_owned(), "pkg-config".to_owned()]);
            assert_eq!(green.add.apk, vec!["libpq-dev".to_owned(), "pkgconf".to_owned()]);
        }

        #[test]
        fn empty_var() {
            use crate::green::validate_csv;
            let vars =
                [(CARGOGREEN_ADD_APT!().to_owned(), "a=1,,b".to_owned())].into_iter().collect();
            let err =
                validate_csv(&mut vec![], CARGOGREEN_ADD_APT!(), &vars).err().unwrap().to_string();
            assert!(err.contains("empty"), "In: {err}");
            assert!(err.contains(CARGOGREEN_ADD_APT), "In: {err}");
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
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
            let green = Green::try_new(manifest, &[].into()).unwrap();
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
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
            let green = Green::try_new(manifest, &[].into()).unwrap();
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
            let green = Green::try_new(manifest, &[].into()).unwrap();
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
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
            let green = Green::try_new(manifest, &[].into()).unwrap();
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
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
            let err = Green::try_new(manifest, &[].into()).err().unwrap().to_string();
            assert!(err.contains("tag"), "In: {err}");
        }
    }
}

#[cfg(test)]
mod from_env {
    use anyhow::Result;

    use super::{Green, Manifest, Vars};
    use crate::all_our_envs::{CARGOGREEN_BASE_IMAGE, CARGOGREEN_REGISTRY_MIRRORS, find_unknowns};

    const MANIFEST: &str = r#"
[package]
name = "test-package"

[package.metadata.green]
registry-mirrors = [ "from.manifest" ]
add.apt = [ "libpq-dev" ]
set-envs = [ "FROM_MANIFEST" ]
"#;

    fn vars(kvs: &[(&str, &str)]) -> Vars {
        kvs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    fn green(kvs: &[(&str, &str)]) -> Result<Green> {
        Green::try_new(Manifest::from_str(MANIFEST).unwrap(), &vars(kvs))
    }

    #[test]
    fn the_manifest_is_used_when_the_environment_is_silent() {
        let green = green(&[]).unwrap();
        assert_eq!(green.registry_mirrors, ["from.manifest"]);
        assert_eq!(green.add.apt, ["libpq-dev"]);
        assert_eq!(green.set_envs, ["FROM_MANIFEST"]);
    }

    #[test]
    fn the_environment_overrides_the_manifest() {
        let green = green(&[
            (CARGOGREEN_REGISTRY_MIRRORS!(), "from.env,other.env"),
            (CARGOGREEN_ADD_APT!(), "libssl-dev"),
            (CARGOGREEN_SET_ENVS!(), "FROM_ENV"),
        ])
        .unwrap();
        assert_eq!(green.registry_mirrors, ["from.env", "other.env"]);
        assert_eq!(green.add.apt, ["libssl-dev"]);
        assert_eq!(green.set_envs, ["FROM_ENV"]);
    }

    #[test]
    fn an_empty_value_resets_rather_than_defaults() {
        assert!(
            green(&[(CARGOGREEN_REGISTRY_MIRRORS!(), "")]).unwrap().registry_mirrors.is_empty()
        );

        let bare = Manifest::from_str("[package]\nname = \"p\"\n").unwrap();
        let green = Green::try_new(bare, &[].into()).unwrap();
        assert!(!green.registry_mirrors.is_empty());
    }

    #[test]
    fn errors_name_the_environment_variable_they_came_from() {
        let err =
            green(&[(CARGOGREEN_REGISTRY_MIRRORS!(), "dupe,dupe")]).err().unwrap().to_string();
        assert!(err.contains(CARGOGREEN_REGISTRY_MIRRORS), "In: {err}");
        assert!(err.contains("duplicates"), "In: {err}");
    }

    #[test]
    fn cache_images_must_be_bare_registry_paths() {
        for (val, wanted) in [
            ("docker-image://myimage", "registry and namespace"),
            ("docker-image://reg.io/ns/img:tag", "must not contain a tag nor digest"),
            (
                "docker-image://reg.io/ns/img@sha256:27086352fd5e1907ea2b934eb1023f217c5ae087992eb59fde121dce9c9ff21e",
                "must not contain a tag nor digest",
            ),
        ] {
            let err = green(&[(CARGOGREEN_CACHE_IMAGES!(), val)]).err().unwrap().to_string();
            assert!(err.contains(wanted), "for {val}, In: {err}");
        }
        let green = green(&[(CARGOGREEN_CACHE_IMAGES!(), "docker-image://reg.io/ns/img")]).unwrap();
        assert_eq!(green.cache.images.len(), 1);
    }

    #[test]
    fn set_envs_may_not_name_our_own_variables() {
        let err =
            green(&[(CARGOGREEN_SET_ENVS!(), "CARGOGREEN_RUNNER")]).err().unwrap().to_string();
        assert!(err.contains("CARGOGREEN_"), "In: {err}");
    }

    #[test]
    fn base_image_must_be_a_valid_uri() {
        let err =
            green(&[(CARGOGREEN_BASE_IMAGE!(), "rust:1.99.0-slim")]).err().unwrap().to_string();
        assert!(err.contains(CARGOGREEN_BASE_IMAGE), "In: {err}");
        assert!(err.contains("scheme"), "In: {err}");

        let green = green(&[(
            CARGOGREEN_BASE_IMAGE!(),
            "docker-image://docker.io/library/rust:1.99.0-slim",
        )])
        .unwrap();
        assert_eq!(green.base.image.noscheme(), "docker.io/library/rust:1.99.0-slim");
    }

    #[test]
    fn misspelled_variables_are_reported() {
        let vars = vars(&[
            ("CARGOGREEN_RUNNER", "none"),
            ("CARGOGREEN_ADD_APTT", "typo"),
            ("CARGOGREEN_NONSENSE", "1"),
            ("PATH", "/usr/bin"),
            ("CARGO_PKG_NAME", "p"),
        ]);
        let mut unknowns = find_unknowns(&vars);
        unknowns.sort();
        assert_eq!(unknowns, ["CARGOGREEN_ADD_APTT", "CARGOGREEN_NONSENSE"]);
    }
}
